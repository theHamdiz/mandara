//! Find a site's feeds: the `<link rel="alternate">` tags of its page, or
//! the addresses blogs conventionally publish them at.

use serde::{Deserialize, Serialize};

use super::html::{Token, Tokens, resolve};

/// A feed offered by a site.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub url: String,
    pub title: String,
}

/// Feed types a page may announce.
const FEED_TYPES: [&str; 5] = ["application/rss+xml", "application/atom+xml", "application/feed+json", "application/json", "application/rdf+xml"];
/// Pages announce a handful of feeds at most; keep the first few.
const MAX_CANDIDATES: usize = 8;
/// Where sites without announcements usually keep their feed.
pub const COMMON_PATHS: [&str; 6] = ["/feed", "/rss.xml", "/atom.xml", "/feed.xml", "/index.xml", "/rss"];

/// The feeds a page announces in its `<head>`, resolved against `base`.
pub fn links(base: &str, page: &str) -> Vec<Candidate> {
    let base = url::Url::parse(base).ok();
    let mut found: Vec<Candidate> = Vec::new();
    for token in Tokens::new(page) {
        match &token {
            Token::Start { name, .. } if name == "link" => {
                let rel = token.attr("rel").unwrap_or_default().to_ascii_lowercase();
                let kind = token.attr("type").unwrap_or_default().trim().to_ascii_lowercase();
                if !rel.split_whitespace().any(|rel| rel == "alternate") || !FEED_TYPES.contains(&kind.as_str()) { continue; }
                // A JSON alternate is only a feed when it says so.
                if kind == "application/json" && !token.attr("href").unwrap_or_default().contains("feed") { continue; }
                let Some(url) = token.attr("href").and_then(|href| resolve(base.as_ref(), href)) else { continue };
                if found.iter().any(|seen| seen.url == url) { continue; }
                let title = token.attr("title").map(|title| super::html::plain_text(title, 120)).unwrap_or_default();
                found.push(Candidate { url, title });
                if found.len() >= MAX_CANDIDATES { break; }
            }
            Token::End(name) if name == "head" && !found.is_empty() => break,
            _ => {}
        }
    }
    // Comment feeds are rarely what anyone means by "the site's feed".
    found.sort_by_key(|candidate| { let lower = candidate.url.to_ascii_lowercase(); lower.contains("comment") });
    found
}

/// The conventional feed addresses of a site, on its own origin.
pub fn common(url: &str) -> Vec<String> {
    let Ok(base) = url::Url::parse(url) else { return Vec::new() };
    COMMON_PATHS.iter().filter_map(|path| base.join(path).ok()).map(String::from).filter(|candidate| candidate != url).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_announce_their_feeds() {
        let page = r#"<!doctype html><html><head><title>Blog</title>
            <link rel="stylesheet" href="/style.css">
            <link rel="alternate" type="application/rss+xml" title="Comments" href="/comments/feed/">
            <LINK REL="alternate" TYPE="application/atom+xml" TITLE="Posts &amp; notes" HREF="atom.xml">
            <link rel="alternate" type="application/feed+json" href="https://example.org/feed.json">
            <link rel="alternate" type="application/json" href="/wp-json/wp/v2/pages/1">
            <link rel="alternate" type="application/rss+xml" href="javascript:alert(1)">
            <link rel="alternate" type="text/html" hreflang="ar" href="/ar/">
            </head><body><link rel="alternate" type="application/rss+xml" href="/late.xml"></body></html>"#;
        let found = links("https://example.org/blog/", page);
        assert_eq!(found.iter().map(|c| c.url.as_str()).collect::<Vec<_>>(), ["https://example.org/blog/atom.xml", "https://example.org/feed.json", "https://example.org/comments/feed/"]);
        assert_eq!(found[0].title, "Posts & notes");
        assert!(links("https://example.org/", "<p>no head</p>").is_empty());
    }

    #[test]
    fn common_paths_stay_on_the_site() {
        let paths = common("https://example.org/some/page");
        assert_eq!(paths[0], "https://example.org/feed");
        assert!(paths.iter().all(|p| p.starts_with("https://example.org/")));
        assert!(common("not a url").is_empty());
    }
}
