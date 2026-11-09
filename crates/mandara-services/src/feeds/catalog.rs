//! Starter feeds by topic. Every address here was fetched and parsed by
//! this reader on [`VERIFIED`] (`cargo test -p mandara-services
//! starter_feeds_fetch_and_parse -- --ignored` checks them again).

use mandara_i18n::tk;

/// When the list was last checked against the live feeds.
pub const VERIFIED: &str = "2026-11-09";

/// One suggested subscription.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Starter {
    pub title: &'static str,
    pub url: &'static str,
    /// Topic key (`tech`, `linux`…); [`topic_name`] gives its label.
    pub topic: &'static str,
}

const fn s(topic: &'static str, title: &'static str, url: &'static str) -> Starter { Starter { title, url, topic } }

/// Topics in the order Studio shows them.
pub const TOPICS: [(&str, &str); 7] = [
    ("tech", tk("Technology")),
    ("linux", tk("Linux & open source")),
    ("dev", tk("Programming")),
    ("science", tk("Science")),
    ("world", tk("World news")),
    ("arabic", tk("Arabic news")),
    ("design", tk("Design & fun")),
];

pub const STARTERS: &[Starter] = &[
    s("tech", "Ars Technica", "https://feeds.arstechnica.com/arstechnica/index"),
    s("tech", "The Verge", "https://www.theverge.com/rss/index.xml"),
    s("tech", "Hacker News", "https://hnrss.org/frontpage"),
    s("tech", "Slashdot", "https://rss.slashdot.org/Slashdot/slashdotMain"),
    s("linux", "LWN.net", "https://lwn.net/headlines/rss"),
    s("linux", "Phoronix", "https://www.phoronix.com/rss.php"),
    s("linux", "Arch Linux news", "https://archlinux.org/feeds/news/"),
    s("linux", "OMG! Ubuntu", "https://www.omgubuntu.co.uk/feed"),
    s("linux", "Planet KDE", "https://planet.kde.org/global/atom.xml"),
    s("dev", "This Week in Rust", "https://this-week-in-rust.org/rss.xml"),
    s("dev", "Rust Blog", "https://blog.rust-lang.org/feed.xml"),
    s("dev", "The GitHub Blog", "https://github.blog/feed/"),
    s("dev", "Julia Evans", "https://jvns.ca/atom.xml"),
    s("science", "Quanta Magazine", "https://api.quantamagazine.org/feed/"),
    s("science", "NASA", "https://www.nasa.gov/feed/"),
    s("science", "Nature", "https://www.nature.com/nature.rss"),
    s("world", "BBC News: World", "https://feeds.bbci.co.uk/news/world/rss.xml"),
    s("world", "DW News", "https://rss.dw.com/rdf/rss-en-all"),
    s("world", "France 24", "https://www.france24.com/en/rss"),
    s("world", "NPR News", "https://feeds.npr.org/1001/rss.xml"),
    s("world", "The Guardian: World", "https://www.theguardian.com/world/rss"),
    s("arabic", "BBC عربي", "https://feeds.bbci.co.uk/arabic/rss.xml"),
    s("arabic", "سكاي نيوز عربية", "https://www.skynewsarabia.com/rss"),
    s("design", "Smashing Magazine", "https://www.smashingmagazine.com/feed/"),
    s("design", "Daring Fireball (JSON Feed)", "https://daringfireball.net/feeds/json"),
    s("design", "xkcd", "https://xkcd.com/atom.xml"),
];

/// A topic's label (English; translate with `tr`).
pub fn topic_name(key: &str) -> &'static str {
    TOPICS.iter().find(|(id, _)| *id == key).map_or("", |(_, name)| name)
}

/// The starter with this address.
pub fn find(url: &str) -> Option<&'static Starter> { STARTERS.iter().find(|starter| starter.url == url) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_starter_has_a_topic_and_a_unique_web_address() {
        let mut seen = std::collections::HashSet::new();
        for starter in STARTERS {
            assert!(super::super::web_url(starter.url) && starter.url.starts_with("https://"), "{}", starter.url);
            assert!(!topic_name(starter.topic).is_empty(), "{}", starter.topic);
            assert!(seen.insert(starter.url), "{} twice", starter.url);
        }
        assert!(TOPICS.iter().all(|(key, _)| STARTERS.iter().any(|s| s.topic == *key)));
    }

    /// The live check behind [`VERIFIED`]: every starter downloads and
    /// parses with entries. Needs the network, so it only runs on request.
    #[test]
    #[ignore = "network"]
    fn starter_feeds_fetch_and_parse() {
        let source = super::super::HttpSource::default();
        let mut failed = Vec::new();
        for starter in STARTERS {
            match super::super::Source::fetch(&source, starter.url, None) {
                Ok(Some(feed)) if !feed.entries.is_empty() => println!("ok   {:>3} entries  {}  ({})", feed.entries.len(), starter.url, feed.title),
                Ok(_) => failed.push(format!("{}: no entries", starter.url)),
                Err(error) => failed.push(format!("{}: {}", starter.url, error.message)),
            }
        }
        assert!(failed.is_empty(), "{failed:#?}");
    }
}
