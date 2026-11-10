//! Fixtures are replies recorded from Reddit's OAuth API (trimmed, with
//! author names replaced). Rate limits and errors use a scripted transport.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::*;

const WANT: Want = Want { thumb: 140, media: 640 };
const LISTING: &[u8] = include_bytes!("fixtures/listing_hot.json");
const PAGE_TWO: &[u8] = include_bytes!("fixtures/listing_page2.json");
const THREAD_SMALL: &[u8] = include_bytes!("fixtures/thread_small.json");
const THREAD_NESTED: &[u8] = include_bytes!("fixtures/thread_nested.json");
const MORE: &[u8] = include_bytes!("fixtures/morechildren.json");
const GALLERY: &[u8] = include_bytes!("fixtures/gallery.json");

#[test]
fn a_recorded_listing_keeps_kinds_flags_thumbnails_and_its_cursor() {
    let page = listing(LISTING, WANT).unwrap();
    assert_eq!(page.posts.len(), 6);
    assert_eq!(page.after.as_deref(), Some("t3_48a6f9"));
    let kinds: Vec<Kind> = page.posts.iter().map(|post| post.kind).collect();
    assert_eq!(kinds, [Kind::Image, Kind::Link, Kind::Video, Kind::Text, Kind::Link, Kind::Text]);
    let image = &page.posts[0];
    assert_eq!(&*image.subreddit, "pics");
    assert_eq!(&*image.author, "fixture_author_1");
    assert!(image.comments > 0 && image.score > 0 && image.created > 1_400_000_000);
    let thumb = image.thumb.as_ref().expect("a preview thumbnail");
    assert!(thumb.url.starts_with("https://") && thumb.width >= WANT.thumb, "{thumb:?}");
    assert!(image.media[0].width >= WANT.media || image.media[0].width < WANT.media && image.media.len() == 1);
    assert!(image.text.is_empty(), "lists never keep self text");
    assert!(image.permalink.starts_with("/r/pics/comments/48dxvx/"));
    assert!(page.posts[4].thumb.is_none(), "a post without a preview has no thumbnail");
    assert!(page.posts[5].nsfw && !page.posts[0].nsfw);
    assert!(page.posts.iter().all(|post| !post.spoiler), "these recordings predate spoiler tags");
}

#[test]
fn a_page_drops_repeated_posts_and_an_unsafe_cursor() {
    let mut value: serde_json::Value = serde_json::from_slice(LISTING).unwrap();
    let first = value["data"]["children"][0].clone();
    value["data"]["children"].as_array_mut().unwrap().push(first);
    value["data"]["after"] = "t3_x&admin=1".into();
    let page = listing(&serde_json::to_vec(&value).unwrap(), WANT).unwrap();
    assert_eq!(page.posts.len(), 6);
    assert!(page.after.is_none());
}

#[test]
fn spoiler_flags_and_long_titles_are_read_and_bounded() {
    let mut value: serde_json::Value = serde_json::from_slice(LISTING).unwrap();
    value["data"]["children"][1]["data"]["spoiler"] = true.into();
    value["data"]["children"][1]["data"]["title"] = format!("line\u{7}one\n{}", "x".repeat(400)).into();
    let page = listing(&serde_json::to_vec(&value).unwrap(), WANT).unwrap();
    assert!(page.posts[1].spoiler);
    let title = &page.posts[1].title;
    assert!(title.starts_with("lineone x"), "{title}");
    assert!(title.chars().count() <= 301 && title.ends_with('…'));
}

#[test]
fn a_recorded_thread_keeps_the_post_text_and_nests_replies_depth_first() {
    let thread = thread(THREAD_SMALL, WANT).unwrap();
    assert_eq!(&*thread.post.id, "2gmzqe");
    assert!(!thread.post.text.is_empty());
    let depths: Vec<u8> = thread.nodes.iter().map(Node::depth).collect();
    assert_eq!(depths.first(), Some(&0));
    assert!(depths.windows(2).all(|pair| pair[1] <= pair[0] + 1), "a reply never skips a level: {depths:?}");
    assert!(depths.contains(&1));
}

#[test]
fn nested_replies_and_more_stubs_come_out_in_reading_order() {
    let thread = thread(THREAD_NESTED, WANT).unwrap();
    let comments = thread.nodes.iter().filter(|node| matches!(node, Node::Comment(_))).count();
    let stubs: Vec<&More> = thread.nodes.iter().filter_map(|node| match node { Node::More(more) => Some(more), _ => None }).collect();
    assert!(comments > 10, "{comments}");
    assert!(stubs.len() >= 2, "a nested stub and the top-level stub");
    let root = stubs.last().unwrap();
    assert_eq!(root.depth, 0);
    assert!(root.count > 0 && !root.children.is_empty());
    assert!(&*root.parent == "t3_3hahrw");
    assert!(thread.nodes.iter().map(Node::depth).max().unwrap() >= 3);
    assert!(!thread.truncated);
    if let Node::Comment(first) = &thread.nodes[0] {
        assert_eq!(first.depth, 0);
        assert!(!first.body.is_empty());
    } else {
        panic!("the thread starts with a comment");
    }
}

#[test]
fn more_children_take_their_depth_from_the_rows_already_shown() {
    let rows = more_children(MORE, |parent| (parent == "3hahrw").then_some(0).or(Some(2)), MAX_COMMENTS).unwrap();
    assert!(!rows.is_empty());
    // Replies to the post itself sit at depth 0 (the post is not a row);
    // a reply to a row in the batch follows it one level deeper.
    for pair in rows.windows(2) {
        assert!(pair[1].depth() <= pair[0].depth() + 1);
    }
    let limited = more_children(MORE, |_| None, 2).unwrap();
    assert_eq!(limited.len(), 2);
}

#[test]
fn a_recorded_gallery_lists_its_images_in_order() {
    let thread = thread(GALLERY, WANT).unwrap();
    assert_eq!(thread.post.kind, Kind::Gallery);
    assert_eq!(thread.post.media.len(), 4);
    assert!(thread.post.media.iter().all(|image| image.url.starts_with("https://preview.redd.it/") || image.url.starts_with("https://i.redd.it/")));
    assert!(thread.post.thumb.is_some());
}

#[test]
fn image_hosts_other_than_reddits_are_never_fetched() {
    let mut value: serde_json::Value = serde_json::from_slice(LISTING).unwrap();
    value["data"]["children"][0]["data"]["preview"]["images"][0]["resolutions"] = serde_json::json!([{ "url": "https://tracker.example/pixel.png", "width": 200, "height": 200 }]);
    value["data"]["children"][0]["data"]["preview"]["images"][0]["source"] = serde_json::json!({ "url": "file:///etc/passwd", "width": 900, "height": 900 });
    let page = listing(&serde_json::to_vec(&value).unwrap(), WANT).unwrap();
    let post = &page.posts[0];
    assert!(post.thumb.as_ref().is_none_or(|thumb| thumb.url.contains("redditmedia.com")), "{:?}", post.thumb);
    assert!(post.media.is_empty());
}

/// The image fetch itself refuses any host but Reddit's, whatever the caller passes.
#[test]
fn the_image_fetch_refuses_other_hosts() {
    let script = Script::default();
    let session = session(&script);
    for url in ["https://tracker.example/pixel.png", "https://i.redd.it.evil.example/x.png", "http://i.redd.it/x.png", "file:///etc/passwd"] {
        assert!(session.image(url).is_err(), "{url}");
    }
    assert!(script.calls.lock().unwrap().is_empty(), "nothing was requested");
}

/// A generated 600-comment thread: every row parsed, depth first, bounded.
#[test]
fn a_large_thread_parses_completely_and_a_huge_one_is_cut_at_the_limit() {
    fn reply(id: usize, depth: usize, width: usize, next: &mut usize) -> serde_json::Value {
        let replies = if depth < 3 {
            let children: Vec<_> = (0..width).map(|_| { *next += 1; reply(*next, depth + 1, width, next) }).collect();
            serde_json::json!({ "kind": "Listing", "data": { "children": children } })
        } else { serde_json::json!("") };
        serde_json::json!({ "kind": "t1", "data": { "id": format!("c{id}"), "author": "fixture", "body": format!("comment {id}"), "score": 1, "created_utc": 1.7e9, "replies": replies } })
    }
    let build = |roots: usize, width: usize| {
        let mut next = 0;
        let children: Vec<_> = (0..roots).map(|_| { next += 1; reply(next, 0, width, &mut next) }).collect();
        let post = serde_json::json!({ "kind": "Listing", "data": { "children": [{ "kind": "t3", "data": { "id": "abc", "title": "A post", "is_self": true, "selftext": "Hello" } }] } });
        (serde_json::to_vec(&serde_json::json!([post, { "kind": "Listing", "data": { "children": children } }])).unwrap(), next)
    };
    let (bytes, total) = build(40, 2);
    assert_eq!(total, 600);
    let thread = thread(&bytes, WANT).unwrap();
    assert_eq!(thread.nodes.len(), 600);
    assert!(!thread.truncated);
    assert_eq!(thread.nodes.iter().map(Node::depth).max(), Some(3));
    let (huge, total) = build(140, 2);
    assert!(total > MAX_COMMENTS);
    let cut = super::thread(&huge, WANT).unwrap();
    assert_eq!(cut.nodes.len(), MAX_COMMENTS);
    assert!(cut.truncated);
}

#[test]
fn paths_cover_sorts_ranges_search_and_cursors() {
    let rust = Subreddit::parse("r/rust").unwrap();
    let mut query = Query { subreddit: rust.clone(), sort: Sort::Hot, span: Span::Day, search: None };
    assert_eq!(query.path(None, 0), "/r/rust/hot?limit=25&raw_json=1");
    assert_eq!(query.path(Some("t3_abc"), 25), "/r/rust/hot?limit=25&raw_json=1&after=t3_abc&count=25");
    assert_eq!(query.path(Some("t3_a&b=c"), 25), "/r/rust/hot?limit=25&raw_json=1", "an unsafe cursor is ignored");
    query.sort = Sort::Top;
    query.span = Span::Week;
    assert_eq!(query.path(None, 0), "/r/rust/top?t=week&limit=25&raw_json=1");
    query.search = Some("async & await?".into());
    assert_eq!(query.path(None, 0), "/r/rust/search?q=async%20%26%20await%3F&restrict_sr=1&type=link&sort=top&t=week&limit=25&raw_json=1");
    assert_eq!(thread_path("abc"), "/comments/abc?limit=500&depth=10&sort=confidence&raw_json=1");
    assert!(more_path("abc", &["d1".into(), "bad,id".into()]).contains("children=d1&"));
}

#[test]
fn community_names_and_post_links_are_validated() {
    for text in ["rust", "r/rust", "/r/rust/", "https://www.reddit.com/r/rust/top/?t=week", "R/rust"] {
        assert_eq!(Subreddit::parse(text).map(|sub| sub.to_string()), Some("r/rust".into()), "{text}");
    }
    for text in ["", "r/", "a", "rust lang", "../etc", "r/abcdefghijklmnopqrstuvwxyz", "rust?x=1&y"] {
        assert!(Subreddit::parse(text).is_none_or(|sub| sub.as_str().bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')), "{text}");
    }
    assert!(Subreddit::parse("rust lang").is_none());
    assert!(Subreddit::parse("x").is_none());
    assert_eq!(post_in_url("https://www.reddit.com/r/rust/comments/1abcd2/some_title/"), Some("1abcd2"));
    assert_eq!(post_in_url("https://old.reddit.com/comments/1abcd2"), Some("1abcd2"));
    assert_eq!(post_in_url("https://redd.it/1abcd2"), Some("1abcd2"));
    assert_eq!(post_in_url("https://example.org/comments/1abcd2"), None);
    assert_eq!(post_id("t3_zz9"), Some("zz9"));
    assert_eq!(post_id("bad/id"), None);
}

#[test]
fn the_budget_waits_for_the_window_and_backs_off_after_429() {
    let now = Instant::now();
    let mut budget = Budget::default();
    assert_eq!(budget.wait(now), None);
    budget.observe(200, Limits { remaining: Some(5.0), reset: Some(30.0), ..Limits::default() }, now);
    assert_eq!(budget.wait(now), None);
    budget.observe(200, Limits { remaining: Some(0.0), reset: Some(30.0), used: Some(100.0), retry_after: None }, now);
    assert_eq!(budget.wait(now), Some(Duration::from_secs(30)));
    assert_eq!(budget.wait(now + Duration::from_secs(31)), None, "a new window starts");
    budget.observe(429, Limits::default(), now);
    let first = budget.wait(now).unwrap();
    budget.observe(429, Limits::default(), now);
    let second = budget.wait(now).unwrap();
    assert!(second > first, "repeated 429s pause longer");
    budget.observe(429, Limits { retry_after: Some(200.0), ..Limits::default() }, now);
    assert!(budget.wait(now).unwrap() >= Duration::from_secs(200), "Retry-After is honoured");
    budget.observe(429, Limits { retry_after: Some(1e9), ..Limits::default() }, now);
    assert!(budget.wait(now).unwrap() <= Duration::from_secs(300), "never longer than five minutes");
    budget.observe(429, Limits { reset: Some(1e30), retry_after: Some(f32::MAX), ..Limits::default() }, now);
    assert!(budget.wait(now).unwrap() <= Duration::from_secs(300), "hints past `Duration::MAX` are clamped, not a panic");
    budget.observe(200, Limits { remaining: Some(50.0), ..Limits::default() }, now);
    assert_eq!(budget.wait(now), None, "a success clears the pause");
    let headers = Limits::from_headers(|name| match name { "x-ratelimit-remaining" => Some("99.0"), "x-ratelimit-reset" => Some(" 412 "), "retry-after" => Some("NaN"), _ => None });
    assert_eq!(headers, Limits { used: None, remaining: Some(99.0), reset: Some(412.0), retry_after: None });
}

/// Scripted replies; every request is recorded.
#[derive(Clone, Default)]
struct Script {
    replies: Arc<Mutex<VecDeque<Response>>>,
    tokens: Arc<Mutex<VecDeque<Response>>>,
    calls: Arc<Mutex<Vec<String>>>,
    /// Times the session closed its idle connections.
    closed: Arc<Mutex<usize>>,
}

fn reply(status: u16, body: &[u8], remaining: f32) -> Response {
    Response { status, limits: Limits { used: Some(1.0), remaining: Some(remaining), reset: Some(60.0), retry_after: None }, body: body.to_vec() }
}

fn token_reply() -> Response {
    Response { status: 200, limits: Limits::default(), body: br#"{"access_token":"fixture-token","token_type":"bearer","expires_in":86400,"scope":"*"}"#.to_vec() }
}

impl Transport for Script {
    fn get(&self, url: &str, bearer: Option<&str>, agent: &str, _: usize) -> Result<Response, String> {
        assert!(agent.starts_with("linux:org.mandara.shell:v"));
        self.calls.lock().unwrap().push(format!("GET {url} {}", bearer.unwrap_or("-")));
        self.replies.lock().unwrap().pop_front().ok_or_else(|| "no scripted reply".into())
    }

    fn token(&self, url: &str, client_id: &str, _: &str) -> Result<Response, String> {
        self.calls.lock().unwrap().push(format!("TOKEN {url} {client_id}"));
        self.tokens.lock().unwrap().pop_front().ok_or_else(|| "no scripted token".into())
    }

    fn close_idle(&mut self) {
        *self.closed.lock().unwrap() += 1;
    }
}

#[test]
fn releasing_forgets_cached_pages_and_closes_idle_connections() {
    let script = Script::default();
    script.tokens.lock().unwrap().push_back(token_reply());
    script.replies.lock().unwrap().extend([reply(200, LISTING, 90.0), reply(200, LISTING, 89.0)]);
    let mut session = session(&script);
    let now = Instant::now();
    session.listing(&query(), None, 0, WANT, now).unwrap();
    session.listing(&query(), None, 0, WANT, now).unwrap();
    let gets = || script.calls.lock().unwrap().iter().filter(|c| c.starts_with("GET")).count();
    assert_eq!(gets(), 1, "the second page came from the cache");
    // The sidebar card went with nothing else reading (setting the client
    // id up already released once).
    let closed = *script.closed.lock().unwrap();
    session.release();
    assert_eq!(*script.closed.lock().unwrap(), closed + 1);
    session.listing(&query(), None, 0, WANT, now).unwrap();
    assert_eq!(gets(), 2, "nothing cached survives a release");
}

fn session(script: &Script) -> Session<Script> {
    let mut session = Session::new(script.clone(), Endpoints::default());
    session.configure("fixture-client", None);
    session
}

fn query() -> Query { Query { subreddit: Subreddit::parse("pics").unwrap(), sort: Sort::Hot, span: Span::Day, search: None } }

#[test]
fn one_token_serves_many_requests_and_a_401_renews_it_once() {
    let script = Script::default();
    script.tokens.lock().unwrap().extend([token_reply(), token_reply()]);
    script.replies.lock().unwrap().extend([reply(200, LISTING, 90.0), reply(401, b"{}", 89.0), reply(200, PAGE_TWO, 88.0)]);
    let mut session = session(&script);
    let now = Instant::now();
    let first = session.listing(&query(), None, 0, WANT, now).unwrap();
    let second = session.listing(&query(), first.after.as_deref(), first.posts.len(), WANT, now).unwrap();
    assert_eq!(second.posts.len(), 4);
    let calls = script.calls.lock().unwrap().clone();
    assert_eq!(calls.iter().filter(|call| call.starts_with("TOKEN")).count(), 2, "{calls:?}");
    assert!(calls[0].starts_with("TOKEN https://www.reddit.com/api/v1/access_token fixture-client"));
    assert!(calls[1].starts_with("GET https://oauth.reddit.com/r/pics/hot?limit=25&raw_json=1 fixture-token"));
    assert!(calls[2].contains("after=t3_48a6f9&count=6"));
    assert!(!format!("{:?}", session.status(now)).contains("fixture-token"));
}

#[test]
fn a_recent_page_is_reused_and_a_new_client_id_forgets_it() {
    let script = Script::default();
    script.tokens.lock().unwrap().extend([token_reply(), token_reply()]);
    script.replies.lock().unwrap().extend([reply(200, LISTING, 90.0), reply(200, LISTING, 89.0)]);
    let mut session = session(&script);
    let now = Instant::now();
    let first = session.listing(&query(), None, 0, WANT, now).unwrap();
    let again = session.listing(&query(), None, 0, WANT, now + Duration::from_secs(5)).unwrap();
    assert!(Arc::ptr_eq(&first, &again));
    assert_eq!(session.status(now).cached_pages, 1);
    session.listing(&query(), None, 0, WANT, now + Duration::from_secs(600)).unwrap();
    assert_eq!(script.calls.lock().unwrap().iter().filter(|call| call.starts_with("GET")).count(), 2, "an old page is fetched again");
    session.configure("another-client", None);
    assert_eq!(session.status(now).cached_pages, 0);
    assert!(!session.status(now).authorized);
}

#[test]
fn rate_limits_pause_requests_without_touching_the_network() {
    let script = Script::default();
    script.tokens.lock().unwrap().push_back(token_reply());
    script.replies.lock().unwrap().extend([reply(200, LISTING, 0.0)]);
    let mut session = session(&script);
    let now = Instant::now();
    session.listing(&query(), None, 0, WANT, now).unwrap();
    let mut other = query();
    other.sort = Sort::New;
    assert!(matches!(session.listing(&other, None, 0, WANT, now), Err(Error::Wait(wait)) if wait == Duration::from_secs(60)));
    assert_eq!(script.calls.lock().unwrap().len(), 2, "no request while the window is used up");
    script.replies.lock().unwrap().push_back(Response { status: 429, limits: Limits { retry_after: Some(20.0), ..Limits::default() }, body: Vec::new() });
    let later = now + Duration::from_secs(61);
    assert!(matches!(session.listing(&other, None, 0, WANT, later), Err(Error::Wait(wait)) if wait >= Duration::from_secs(20)));
    assert!(session.status(later).paused_for_seconds.is_some());
}

#[test]
fn private_and_missing_communities_are_told_apart() {
    let script = Script::default();
    script.tokens.lock().unwrap().push_back(token_reply());
    script.replies.lock().unwrap().extend([reply(403, br#"{"reason": "private", "message": "Forbidden", "error": 403}"#, 80.0), reply(404, br#"{"reason": "banned"}"#, 79.0)]);
    let mut session = session(&script);
    let now = Instant::now();
    assert_eq!(session.listing(&query(), None, 0, WANT, now).unwrap_err(), Error::Missing { private: true });
    assert_eq!(session.thread("abc", None, WANT, now).unwrap_err(), Error::Missing { private: false });
    assert!(!Error::Setup.to_string().is_empty());
}

const RSS_LISTING: &[u8] = include_bytes!("fixtures/rss_listing.xml");
const RSS_COMMENTS: &[u8] = include_bytes!("fixtures/rss_comments.xml");

fn feed_reply(body: &[u8]) -> Response {
    Response { status: 200, limits: Limits { used: Some(1.0), remaining: Some(0.0), reset: Some(30.0), retry_after: None }, body: body.to_vec() }
}

#[test]
fn a_recorded_community_feed_gives_limited_posts() {
    let page = rss::listing(RSS_LISTING).unwrap();
    assert!(page.limited && page.posts.len() == 6 && page.after.is_none(), "a short page has no next page");
    assert!(page.posts.iter().all(|post| post.limited && post.score == 0 && post.comments == 0 && &*post.subreddit == "linux" && post.created > 1_700_000_000));
    assert!(page.posts.iter().all(|post| post.author.starts_with("fixture_author_") && post.permalink.starts_with("/r/linux/comments/")));
    let picture = page.posts.iter().find(|post| post.kind == Kind::Image).expect("a picture post");
    assert!(picture.media[0].url.starts_with("https://i.redd.it/") && picture.thumb.is_some());
    let text = page.posts.iter().find(|post| post.kind == Kind::Text).expect("a text post");
    assert!(text.url.is_none() && text.text.is_empty(), "lists never keep self text");
    let link = page.posts.iter().find(|post| post.kind == Kind::Link).expect("a link post");
    assert!(link.url.as_deref().is_some_and(|url| url.starts_with("https://")) && !link.domain.is_empty());
}

#[test]
fn a_recorded_comments_feed_gives_the_post_and_flat_comments() {
    let thread = rss::thread(RSS_COMMENTS).unwrap();
    assert!(thread.limited && thread.post.limited);
    assert_eq!(&*thread.post.id, "1xnyh98");
    assert_eq!(thread.nodes.len(), 6);
    for node in &thread.nodes {
        let Node::Comment(comment) = node else { panic!("feeds have no stubs") };
        assert_eq!(comment.depth, 0);
        assert!(comment.score_hidden && !comment.body.is_empty() && !comment.body.contains("<p>") && !comment.body.contains("&#39;"), "{}", comment.body);
    }
}

#[test]
fn feed_html_becomes_safe_markdown() {
    let html = r#"<!-- SC_OFF --><div class="md"><p>It&#39;s <strong>bold</strong>, <em>soft</em> &amp; <code>x*y</code>.</p><ul><li>one</li><li>two</li></ul><blockquote><p>quoted</p></blockquote><p><a href="https://example.org/a?b=1&amp;c=2">a link</a> and <span class="md-spoiler-text">secret</span> and 2*3 <script>alert(1)</script></p><pre><code>let a = 1;</code></pre></div><!-- SC_ON -->"#;
    let text = rss::markdown(html);
    assert!(text.contains("It's **bold**, *soft* & `x*y`."), "{text}");
    assert!(text.contains("- one") && text.contains("- two") && text.contains("> quoted"), "{text}");
    assert!(text.contains("[a link](https://example.org/a?b=1&c=2)") && text.contains(">!secret!<"), "{text}");
    assert!(text.contains("2\\*3") && !text.contains("<script>") && text.contains("alert(1)"), "tags are dropped, text kept and escaped: {text}");
    assert!(text.contains("```\nlet a = 1;\n```"), "{text}");
    // Non-ASCII text right after `&` (byte 12 inside a character) is kept, not a panic.
    assert!(rss::markdown("<p>a & xمرحبا</p>").contains("a & xمرحبا"));
}

#[test]
fn feed_paths_cover_sorts_search_and_permalinks() {
    let mut query = query();
    assert_eq!(rss::listing_path(&query, None), "/r/pics/hot/.rss?limit=25");
    query.sort = Sort::Top;
    query.span = Span::Week;
    assert_eq!(rss::listing_path(&query, Some("t3_abc")), "/r/pics/top/.rss?t=week&limit=25&after=t3_abc");
    query.search = Some("dark mode".into());
    assert!(rss::listing_path(&query, None).starts_with("/r/pics/search.rss?q=dark%20mode&restrict_sr=on&sort=top&t=week&"));
    assert_eq!(rss::comments_path("abc", Some("/r/pics/comments/abc/title/")), "/r/pics/comments/abc/title/.rss?limit=100");
    assert_eq!(rss::comments_path("abc", Some("https://evil.example/")), "/comments/abc/.rss?limit=100");
}

/// One feed request a minute, never a burst, longer when Reddit asks.
#[test]
fn the_feed_queue_allows_one_request_a_minute_and_honours_pauses() {
    let now = Instant::now();
    let mut pacer = Pacer::default();
    assert_eq!(pacer.wait(now), None);
    pacer.sent(now);
    assert_eq!(pacer.wait(now), Some(rss::RSS_INTERVAL));
    pacer.observe(200, Limits { remaining: Some(0.0), reset: Some(30.0), ..Limits::default() }, now);
    assert_eq!(pacer.wait(now), Some(rss::RSS_INTERVAL), "a shorter reset never shortens the interval");
    pacer.observe(429, Limits { retry_after: Some(150.0), ..Limits::default() }, now);
    assert_eq!(pacer.wait(now), Some(Duration::from_secs(150)));
    assert_eq!(pacer.wait(now + Duration::from_secs(151)), None);
    pacer.sent(now + Duration::from_secs(151));
    pacer.sent(now + Duration::from_secs(152));
    assert!(pacer.wait(now + Duration::from_secs(152)).unwrap() >= Duration::from_secs(59), "a second send never shortens the wait");
    pacer.observe(429, Limits { retry_after: Some(1e9), ..Limits::default() }, now);
    assert!(pacer.wait(now).unwrap() <= Duration::from_secs(600));
    // Past `Duration::MAX`: clamped, not a panic.
    pacer.observe(429, Limits { remaining: Some(0.0), reset: Some(1e30), retry_after: Some(f32::MAX), ..Limits::default() }, now);
    assert!(pacer.wait(now).unwrap() <= Duration::from_secs(600));
}

#[test]
fn without_an_app_the_session_reads_feeds_once_a_minute_and_caches_them() {
    let script = Script::default();
    script.replies.lock().unwrap().extend([feed_reply(RSS_LISTING), feed_reply(RSS_COMMENTS)]);
    let mut session = Session::new(script.clone(), Endpoints::default());
    let now = Instant::now();
    let page = session.listing(&query(), None, 0, WANT, now).unwrap();
    assert!(page.limited && session.status(now).limited);
    assert!(matches!(session.thread("1xnyh98", Some("/r/linux/comments/1xnyh98/title/"), WANT, now + Duration::from_secs(10)), Err(Error::Wait(wait)) if wait <= rss::RSS_INTERVAL));
    assert!(session.listing(&query(), None, 0, WANT, now + Duration::from_secs(20)).is_ok(), "a cached feed needs no request");
    let thread = session.thread("1xnyh98", Some("/r/linux/comments/1xnyh98/title/"), WANT, now + Duration::from_secs(61)).unwrap();
    assert!(thread.limited);
    session.thread("1xnyh98", Some("/r/linux/comments/1xnyh98/title/"), WANT, now + Duration::from_secs(200)).unwrap();
    let calls = script.calls.lock().unwrap().clone();
    assert_eq!(calls, ["GET https://www.reddit.com/r/pics/hot/.rss?limit=25 -", "GET https://www.reddit.com/r/linux/comments/1xnyh98/title/.rss?limit=100 -"], "no token, no burst, the cache answers the rest");
}

#[test]
fn a_refused_app_falls_back_to_limited_mode_until_the_client_id_changes() {
    let script = Script::default();
    script.tokens.lock().unwrap().push_back(Response { status: 401, limits: Limits::default(), body: br#"{"message": "Unauthorized", "error": 401}"#.to_vec() });
    script.replies.lock().unwrap().extend([feed_reply(RSS_LISTING)]);
    let mut session = session(&script);
    let now = Instant::now();
    assert!(session.listing(&query(), None, 0, WANT, now).unwrap().limited);
    assert!(session.status(now).limited);
    session.listing(&query(), None, 0, WANT, now + Duration::from_secs(5)).unwrap();
    assert_eq!(script.calls.lock().unwrap().iter().filter(|call| call.starts_with("TOKEN")).count(), 1, "a refused id is not asked again");
    session.configure("another-client", None);
    assert!(!session.is_limited());
}

/// The installed-client grant has no refresh token: a new token is simply
/// requested shortly before the old one expires, and a failed renewal reads
/// the feeds for that request instead of failing.
#[test]
fn tokens_renew_before_expiry_and_a_failed_renewal_uses_limited_mode() {
    let script = Script::default();
    let short = Response { status: 200, limits: Limits::default(), body: br#"{"access_token":"fixture-token","expires_in":600}"#.to_vec() };
    script.tokens.lock().unwrap().extend([short, Response { status: 503, limits: Limits::default(), body: Vec::new() }, token_reply()]);
    script.replies.lock().unwrap().extend([reply(200, LISTING, 90.0), reply(200, PAGE_TWO, 89.0), feed_reply(RSS_LISTING), reply(200, LISTING, 88.0)]);
    let mut session = session(&script);
    let now = Instant::now();
    let mut other = query();
    session.listing(&query(), None, 0, WANT, now).unwrap();
    other.sort = Sort::New;
    session.listing(&other, None, 0, WANT, now + Duration::from_secs(500)).unwrap();
    assert_eq!(script.calls.lock().unwrap().iter().filter(|call| call.starts_with("TOKEN")).count(), 1, "still valid a minute before expiry");
    other.sort = Sort::Rising;
    let fallback = session.listing(&other, None, 0, WANT, now + Duration::from_secs(560)).unwrap();
    assert!(fallback.limited, "a failed renewal reads the feed instead of showing an error");
    assert!(!session.is_limited(), "and the app is tried again next time");
    other.sort = Sort::Top;
    assert!(!session.listing(&other, None, 0, WANT, now + Duration::from_secs(620)).unwrap().limited);
    assert_eq!(script.calls.lock().unwrap().iter().filter(|call| call.starts_with("TOKEN")).count(), 3);
}

/// A token lifetime past `Duration::MAX` is clamped, not a panic.
#[test]
fn a_huge_token_lifetime_is_clamped() {
    let script = Script::default();
    let huge = Response { status: 200, limits: Limits::default(), body: br#"{"access_token":"fixture-token","expires_in":1e300}"#.to_vec() };
    script.tokens.lock().unwrap().extend([huge, token_reply()]);
    script.replies.lock().unwrap().extend([reply(200, LISTING, 90.0), reply(200, LISTING, 89.0)]);
    let mut session = session(&script);
    let now = Instant::now();
    session.listing(&query(), None, 0, WANT, now).unwrap();
    let mut other = query();
    other.sort = Sort::New;
    session.listing(&other, None, 0, WANT, now + Duration::from_secs(7200)).unwrap();
    assert_eq!(script.calls.lock().unwrap().iter().filter(|call| call.starts_with("TOKEN")).count(), 2, "renewed within the hour");
}

#[test]
fn endpoints_only_accept_loopback_overrides_and_user_agents_follow_reddits_format() {
    assert_eq!(Endpoints::local("http://127.0.0.1:8080/"), Some(Endpoints { api: "http://127.0.0.1:8080".into(), token: "http://127.0.0.1:8080/api/v1/access_token".into(), site: "http://127.0.0.1:8080".into() }));
    assert_eq!(Endpoints::local("http://evil.example"), None);
    assert_eq!(Endpoints::local("https://127.0.0.1"), None);
    assert!(user_agent(Some("mandara_user")).ends_with("(by /u/mandara_user)"));
    assert!(user_agent(Some("bad name)")).contains("github.com/theHamdiz/mandara"));
}

#[test]
fn hidden_replies_and_continued_threads_are_requested_with_validated_ids() {
    let script = Script::default();
    script.tokens.lock().unwrap().push_back(token_reply());
    script.replies.lock().unwrap().extend([reply(200, MORE, 90.0), reply(200, THREAD_NESTED, 89.0)]);
    let mut session = session(&script);
    let now = Instant::now();
    let rows = session.more("3hahrw", &["cu5tead".into()], |_| Some(0), 10, now).unwrap();
    assert!(!rows.is_empty() && rows.len() <= 10);
    let thread = thread(THREAD_NESTED, WANT).unwrap();
    let Some(Node::Comment(first)) = thread.nodes.first() else { panic!() };
    let continued = session.continue_thread("3hahrw", &format!("t1_{}", first.id), 1, 50, now).unwrap();
    assert!(continued.iter().all(|node| node.depth() >= 1));
    let calls = script.calls.lock().unwrap();
    assert!(calls[1].contains("/api/morechildren?api_type=json&link_id=t3_3hahrw&children=cu5tead&"));
    assert!(calls[2].contains(&format!("/comments/3hahrw/_/{}?", first.id)));
}
