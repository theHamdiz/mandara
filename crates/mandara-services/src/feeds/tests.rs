use std::io::Write as _;
use std::net::TcpListener;

use super::*;

const RSS: &str = r#"<rss version="2.0"><channel><title>Coastal news</title><link>https://example.org</link><description>News</description><item><guid>one</guid><title>Sea &amp; sky</title><link>https://example.org/one</link><pubDate>Wed, 04 Nov 2026 10:00:00 +0000</pubDate></item><item><title>Duplicate</title><link>https://example.org/one</link></item><item><title>Unsafe link</title><link>file:///etc/passwd</link></item></channel></rss>"#;

#[test]
fn rss_keeps_real_web_entries_and_publication_times() {
    let feed = parse("https://example.org/feed", RSS.as_bytes()).unwrap();
    assert_eq!(feed.title, "Coastal news");
    assert_eq!(feed.entries.len(), 1);
    assert_eq!(feed.entries[0].title, "Sea & sky");
    assert_eq!(feed.entries[0].id, "one");
    assert_eq!(feed.entries[0].published, Some(1_793_786_400));
}

#[test]
fn atom_resolves_relative_links_and_ignores_enclosures() {
    let xml = r#"<feed xmlns="http://www.w3.org/2005/Atom" xml:base="https://example.org/articles/"><title>Journal</title><id>journal</id><updated>2026-11-04T10:00:00Z</updated><entry><title>A morning</title><id>morning</id><updated>2026-11-04T10:00:00Z</updated><link rel="enclosure" href="audio.mp3"/><link rel="alternate" href="morning"/></entry></feed>"#;
    let feed = parse("https://example.org/feed", xml.as_bytes()).unwrap();
    assert_eq!(feed.entries[0].link, "https://example.org/articles/morning");
    assert!(feed.entries[0].published.is_some());
}

#[test]
fn json_feeds_share_the_reader_model() {
    let json = br#"{"version":"https://jsonfeed.org/version/1.1","title":"Journal","items":[{"id":"post","url":"https://example.org/post","title":"A day","content_text":"Day"}]}"#;
    assert_eq!(parse("https://example.org/feed.json", json).unwrap().entries[0].title, "A day");
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
    assert_eq!(feed.entries[0].title.chars().count(), 513);
    assert!(feed.entries[0].title.ends_with('…'));
}

#[test]
fn only_uncredentialed_web_urls_can_be_opened() {
    for value in ["https://example.org/post", "http://localhost:8080/feed"] {
        assert!(web_url(value), "{value}");
    }
    for value in ["file:///etc/passwd", "javascript:alert(1)", "https://user:secret@example.org/feed", "/post", "https://"] {
        assert!(!web_url(value), "{value}");
    }
}

#[test]
fn conditional_requests_keep_the_cached_content() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/feed", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        for (i, response) in [format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"news-1\"\r\nConnection: close\r\n\r\n{RSS}", RSS.len()), "HTTP/1.1 304 Not Modified\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into()].into_iter().enumerate() {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(REQUEST_TIMEOUT)).unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 1024];
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let n = stream.read(&mut chunk).unwrap();
                assert_ne!(n, 0, "the client closed before sending its headers");
                request.extend_from_slice(&chunk[..n]);
                assert!(request.len() <= 8192, "the fixture only accepts small feed requests");
            }
            let request = String::from_utf8_lossy(&request);
            if i == 1 { assert!(request.to_ascii_lowercase().contains("if-none-match: \"news-1\"")); }
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    let source = HttpSource { agent: ureq::Agent::config_builder().timeout_global(Some(REQUEST_TIMEOUT)).proxy(None).build().into() };
    let feed = source.fetch_online(&url, None).unwrap().unwrap();
    assert_eq!(feed.etag.as_deref(), Some("\"news-1\""));
    assert!(source.fetch_online(&url, Some(&feed)).unwrap().is_none());
    worker.join().unwrap();
}
