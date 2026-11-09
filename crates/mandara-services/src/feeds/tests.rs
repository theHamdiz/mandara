use std::io::{Read as _, Write as _};
use std::net::TcpListener;

use super::*;

const RSS2: &[u8] = include_bytes!("fixtures/rss2.xml");
const RSS1: &[u8] = include_bytes!("fixtures/rss1.rdf");
const ATOM: &[u8] = include_bytes!("fixtures/atom.xml");
const JSON: &[u8] = include_bytes!("fixtures/feed.json");
const QUIRKS: &[u8] = include_bytes!("fixtures/quirks.xml");

fn titles(feed: &Feed) -> Vec<&str> { feed.entries.iter().map(|entry| entry.title.as_str()).collect() }

#[test]
fn rss2_keeps_real_web_entries_pictures_authors_and_content() {
    let parsed = parse_full("https://example.org/feed", RSS2, None).unwrap();
    let feed = &parsed.feed;
    assert_eq!(feed.title, "Coastal & Harbour News");
    assert_eq!(feed.icon.as_deref(), Some("https://example.org/icon.png"));
    assert_eq!(titles(feed), ["Fishing boats return early", "Lighthouse – restored", "An item with only a permalink guid and no title at all"]);
    let boats = &feed.entries[0];
    assert_eq!(boats.id, "boats-2026");
    assert_eq!(boats.published, Some(1_793_779_200), "+0200 converts to UTC");
    assert_eq!(boats.author.as_deref(), Some("Nour Hassan"));
    assert_eq!(boats.image.as_deref(), Some("https://example.org/thumbs/boats.jpg"));
    assert_eq!(boats.summary, "The fleet came back before noon.");
    assert_eq!(feed.entries[1].link, "https://example.org/lighthouse", "relative links resolve");
    assert_eq!(feed.entries[1].image.as_deref(), Some("https://example.org/photos/lighthouse.jpg"), "image enclosures are thumbnails");
    assert_eq!(feed.entries[2].link, "https://example.org/guid-only");
    let article = &parsed.articles[0];
    assert_eq!(article.images().collect::<Vec<_>>(), ["https://example.org/img/boats.jpg"]);
    assert!(!format!("{article:?}").contains("track"));
}

#[test]
fn rss1_rdf_in_latin1_reads_with_its_dates_and_authors() {
    let feed = parse("https://news.example/rss", RSS1).unwrap();
    assert_eq!(feed.title, "RDF Daily");
    assert_eq!(titles(&feed), ["Café culture returns", "Second story"]);
    assert_eq!(feed.entries[0].published, Some(1_793_607_300));
    assert_eq!(feed.entries[0].author.as_deref(), Some("The Editor"));
    assert_eq!(feed.entries[1].published, None);
}

#[test]
fn atom_resolves_relative_links_skips_enclosures_and_clamps_dates() {
    let parsed = parse_full("https://journal.example/atom.xml", ATOM, None).unwrap();
    let feed = &parsed.feed;
    assert_eq!(feed.title, "Journal of Light");
    assert_eq!(feed.site.as_deref(), Some("https://journal.example/"));
    assert_eq!(feed.icon.as_deref(), Some("https://journal.example/favicon.png"));
    assert_eq!(feed.entries[0].link, "https://journal.example/articles/morning");
    assert_eq!(feed.entries[0].author.as_deref(), Some("Salma"));
    assert_eq!(feed.entries[0].summary, "Sun over the harbour.");
    assert!(feed.entries[0].published.is_some());
    assert_eq!(feed.entries[1].published, None, "a date in 2099 is a broken clock");
    let text = format!("{:?}", parsed.articles[0]);
    assert!(text.contains("Walking") && !text.contains("ads.example"), "{text}");
}

#[test]
fn json_feeds_share_the_reader_model() {
    let parsed = parse_full("https://json.example/feed.json", JSON, None).unwrap();
    let feed = &parsed.feed;
    assert_eq!(feed.title, "JSON Journal");
    assert_eq!(feed.entries[0].image.as_deref(), Some("https://json.example/one.jpg"));
    assert_eq!(feed.entries[0].author.as_deref(), Some("Karim"));
    assert!(feed.entries[1].title.starts_with("Untitled note"), "untitled entries use their text");
    assert_eq!(parsed.articles[1].blocks.len(), 2, "plain text keeps its paragraphs");
}

#[test]
fn quirky_feeds_with_junk_html_entities_and_bare_ampersands_still_read() {
    let feed = parse("https://quirks.example/rss", QUIRKS).unwrap();
    assert_eq!(feed.title, "Quirks & Co");
    assert_eq!(feed.entries[0].title, "Tom & Jerry — AT&T …");
    assert_eq!(feed.entries[0].summary, "Escaped markup");
    assert!(feed.entries[0].published.is_some(), "single-digit RFC 822 times parse");
    assert_eq!(feed.entries[1].published, None);
}

#[test]
fn malformed_and_oversized_feeds_report_errors() {
    assert!(parse("https://example.org/feed", b"this is not a feed").is_err());
    assert!(parse("https://example.org/feed", &vec![b' '; MAX_FEED_BYTES + 1]).is_err());
}

#[test]
fn large_headlines_are_bounded_on_character_boundaries() {
    let value = serde_json::json!({"version":"https://jsonfeed.org/version/1.1","title":"News", "items":[{"id":"one", "url":"https://example.org/one", "title":"界".repeat(1000), "content_text":"Story"}]});
    let feed = parse("https://example.org/feed", &serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(feed.entries[0].title.chars().count() <= 513);
    assert!(feed.entries[0].title.ends_with('…'));
}

/// Feed links permit ordinary web URLs, never embedded credentials or local-file schemes.
#[test]
fn only_uncredentialed_web_urls_can_be_opened() {
    for value in ["https://example.org/post", "http://localhost:8080/feed"] {
        assert!(web_url(value), "{value}");
    }
    for value in ["file:///etc/passwd", "javascript:alert(1)", "https://user:secret@example.org/feed", "/post", "https://"] {
        assert!(!web_url(value), "{value}");
    }
    assert_eq!(normalize_address(" example.org/feed ").unwrap(), "https://example.org/feed");
    assert!(normalize_address("not an address").is_err());
}

/// A local server answering each connection with the next canned
/// response; returns the requests it saw.
fn serve(responses: Vec<Vec<u8>>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for response in responses {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(http::REQUEST_TIMEOUT)).unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 1024];
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let n = stream.read(&mut chunk).unwrap();
                assert_ne!(n, 0, "the client closed before sending its headers");
                request.extend_from_slice(&chunk[..n]);
                assert!(request.len() <= 8192, "the fixture only accepts small requests");
            }
            seen.push(String::from_utf8_lossy(&request).to_ascii_lowercase());
            stream.write_all(&response).unwrap();
        }
        seen
    });
    (base, worker)
}

fn ok(headers: &str, body: &[u8]) -> Vec<u8> {
    let mut response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n", body.len()).into_bytes();
    response.extend_from_slice(body);
    response
}

fn local() -> HttpSource { HttpSource { agent: ureq::Agent::config_builder().timeout_global(Some(http::REQUEST_TIMEOUT)).max_redirects(0).http_status_as_error(false).proxy(None).build().into() } }

/// An unchanged feed sends its ETag and preserves the previously parsed content.
#[test]
fn conditional_requests_keep_the_cached_content() {
    let (base, worker) = serve(vec![ok("ETag: \"news-1\"\r\nLast-Modified: Wed, 04 Nov 2026 10:00:00 GMT\r\n", RSS2), b"HTTP/1.1 304 Not Modified\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()]);
    let url = format!("{base}/feed");
    let source = local();
    let feed = source.fetch_online(&url, None).unwrap().unwrap();
    assert_eq!(feed.etag.as_deref(), Some("\"news-1\""));
    assert!(source.fetch_online(&url, Some(&feed)).unwrap().is_none());
    let requests = worker.join().unwrap();
    assert!(requests[1].contains("if-none-match: \"news-1\"") && requests[1].contains("if-modified-since: wed, 04 nov 2026"));
    assert!(requests[0].contains("accept-encoding: gzip"));
}

#[test]
fn gzipped_feeds_decompress() {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(ATOM).unwrap();
    let (base, worker) = serve(vec![ok("Content-Encoding: gzip\r\nContent-Type: application/atom+xml\r\n", &gz.finish().unwrap())]);
    let feed = local().fetch_online(&format!("{base}/atom"), None).unwrap().unwrap();
    assert_eq!(feed.title, "Journal of Light");
    worker.join().unwrap();
}

#[test]
fn redirects_are_followed_checked_and_remembered_when_permanent() {
    let (base, worker) = serve(vec![b"HTTP/1.1 301 Moved Permanently\r\nLocation: /new-feed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(), ok("", RSS2)]);
    let url = format!("{base}/old-feed");
    let feed = local().fetch_online(&url, None).unwrap().unwrap();
    assert_eq!(feed.url, url, "the subscription keeps its own key");
    assert_eq!(feed.resolved.as_deref(), Some(format!("{base}/new-feed").as_str()));
    let requests = worker.join().unwrap();
    assert!(requests[1].starts_with("get /new-feed "));
}

/// Relative links resolve against the address that served the feed (after
/// a redirect, or the feed a page announces), not the subscription key.
#[test]
fn relative_links_resolve_against_the_served_address() {
    let relative = b"<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><title>Relative</title><item><title>One</title><link>post-1</link><guid>one</guid></item></channel></rss>";
    let (base, worker) = serve(vec![b"HTTP/1.1 301 Moved Permanently\r\nLocation: /new/feed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(), ok("", relative)]);
    let url = format!("{base}/old/feed");
    let feed = local().fetch_online(&url, None).unwrap().unwrap();
    assert_eq!(feed.url, url, "the subscription keeps its own key");
    assert_eq!(feed.entries.iter().map(|entry| entry.link.as_str()).collect::<Vec<_>>(), [format!("{base}/new/post-1")]);
    worker.join().unwrap();
    let page = b"<!doctype html><html><head><link rel=\"alternate\" type=\"application/rss+xml\" href=\"/feeds/main.xml\"></head><body>Blog</body></html>";
    let (base, worker) = serve(vec![ok("Content-Type: text/html\r\n", page), ok("", relative)]);
    let url = format!("{base}/blog/");
    let feed = local().fetch_online(&url, None).unwrap().unwrap();
    assert_eq!(feed.url, url);
    assert_eq!(feed.entries.iter().map(|entry| entry.link.as_str()).collect::<Vec<_>>(), [format!("{base}/feeds/post-1")]);
    worker.join().unwrap();
}

#[test]
fn redirects_to_unsafe_addresses_or_in_loops_fail() {
    let (base, worker) = serve(vec![b"HTTP/1.1 302 Found\r\nLocation: file:///etc/passwd\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()]);
    let error = local().fetch_online(&format!("{base}/feed"), None).unwrap_err();
    assert!(error.message.contains("http"), "{}", error.message);
    worker.join().unwrap();
    let hop = b"HTTP/1.1 302 Found\r\nLocation: /loop\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec();
    let (base, worker) = serve(vec![hop; http::MAX_REDIRECTS + 1]);
    let error = local().fetch_online(&format!("{base}/loop"), None).unwrap_err();
    assert!(error.message.contains("too many"), "{}", error.message);
    worker.join().unwrap();
}

#[test]
fn busy_servers_say_when_to_come_back() {
    let (base, worker) = serve(vec![b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 1200\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()]);
    let error = local().fetch_online(&format!("{base}/feed"), None).unwrap_err();
    assert_eq!(error.retry_after, Some(1200));
    worker.join().unwrap();
}

#[test]
fn a_web_page_is_read_through_the_feed_it_announces() {
    let page = b"<!doctype html><html><head><link rel=\"alternate\" type=\"application/rss+xml\" title=\"Posts\" href=\"/feed.xml\"></head><body>Blog</body></html>";
    let (base, worker) = serve(vec![ok("Content-Type: text/html\r\n", page), ok("", RSS2)]);
    let url = format!("{base}/");
    let feed = local().fetch_online(&url, None).unwrap().unwrap();
    assert_eq!(feed.url, url);
    assert_eq!(feed.resolved.as_deref(), Some(format!("{base}/feed.xml").as_str()));
    assert_eq!(feed.entries.len(), 3);
    worker.join().unwrap();
}
