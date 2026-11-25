//! OPML, the file feed readers swap subscription lists in. Folders are
//! outlines holding outlines; feeds are outlines with an `xmlUrl`.

use mandara_i18n::t;

use super::html::{Token, Tokens};
use super::{Subscription, web_url};

/// An imported list larger than this is not a reading list.
pub const MAX_IMPORT_BYTES: usize = 2 * 1024 * 1024;

/// The subscriptions an OPML document lists, in order, each with the
/// folder it sits in (nested folders join with " / ").
pub fn import(text: &str) -> Result<Vec<Subscription>, String> {
    if text.len() > MAX_IMPORT_BYTES { return Err(t("This OPML file is too large").into()); }
    if !text.to_ascii_lowercase().contains("<opml") { return Err(t("This is not an OPML file").into()); }
    let mut folders: Vec<Option<String>> = Vec::new();
    let mut found: Vec<Subscription> = Vec::new();
    for token in Tokens::new(text) {
        match &token {
            Token::Start { name, closed, .. } if name == "outline" => {
                let label = token.attr("title").or_else(|| token.attr("text")).map(|label| label.trim().to_owned()).unwrap_or_default();
                match token.attr("xmlurl").map(str::trim) {
                    Some(url) if web_url(url) => {
                        if !found.iter().any(|seen| seen.url == url) {
                            let folder = folders.iter().flatten().cloned().collect::<Vec<_>>().join(" / ");
                            found.push(Subscription { url: url.to_owned(), title: label, folder, color: String::new() });
                        }
                        if !closed { folders.push(None); }
                    }
                    Some(_) => { if !closed { folders.push(None); } }
                    None if !closed => folders.push((!label.is_empty()).then_some(label)),
                    None => {}
                }
            }
            Token::End(name) if name == "outline" => { folders.pop(); }
            _ => {}
        }
    }
    if found.is_empty() { return Err(t("This OPML file lists no feeds").into()); }
    Ok(found)
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

/// An OPML 2.0 document of `subscriptions`, folders in first-use order.
pub fn export(subscriptions: &[Subscription]) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<opml version=\"2.0\">\n  <head>\n    <title>Mandara Feeds</title>\n  </head>\n  <body>\n");
    let feed = |out: &mut String, sub: &Subscription, indent: &str| {
        let title = escape(if sub.title.is_empty() { &sub.url } else { &sub.title });
        out.push_str(&format!("{indent}<outline type=\"rss\" text=\"{title}\" title=\"{title}\" xmlUrl=\"{}\"/>\n", escape(&sub.url)));
    };
    let mut folders: Vec<&str> = Vec::new();
    for sub in subscriptions {
        if sub.folder.is_empty() { feed(&mut out, sub, "    "); } else if !folders.contains(&sub.folder.as_str()) { folders.push(&sub.folder); }
    }
    for folder in folders {
        out.push_str(&format!("    <outline text=\"{0}\" title=\"{0}\">\n", escape(folder)));
        for sub in subscriptions.iter().filter(|sub| sub.folder == folder) { feed(&mut out, sub, "      "); }
        out.push_str("    </outline>\n");
    }
    out.push_str("  </body>\n</opml>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(url: &str, title: &str, folder: &str) -> Subscription {
        Subscription { url: url.into(), title: title.into(), folder: folder.into(), color: String::new() }
    }

    #[test]
    fn opml_round_trips_folders_titles_and_escaping() {
        let list = vec![sub("https://a.example/feed", "A & B", ""), sub("https://b.example/rss?x=1&y=2", "Bee \"news\"", "Tech"), sub("https://c.example/atom", "", "Tech"), sub("https://d.example/feed", "عربي", "أخبار")];
        let text = export(&list);
        assert!(text.contains("xmlUrl=\"https://b.example/rss?x=1&amp;y=2\""));
        let back = import(&text).unwrap();
        assert_eq!(back[0], list[0]);
        assert_eq!(back.iter().map(|s| (s.url.as_str(), s.folder.as_str())).collect::<Vec<_>>(), [("https://a.example/feed", ""), ("https://b.example/rss?x=1&y=2", "Tech"), ("https://c.example/atom", "Tech"), ("https://d.example/feed", "أخبار")]);
        assert_eq!(back[1].title, "Bee \"news\"");
        assert_eq!(back[2].title, "https://c.example/atom", "an untitled feed exports its address as its name");
        assert_eq!(back[3].title, "عربي");
    }

    #[test]
    fn other_readers_exports_import_with_nested_folders() {
        let text = r#"<?xml version="1.0"?><opml version="1.0"><head><title>x</title></head><body>
            <outline text="News"><outline text="World"><outline type="rss" text="BBC" xmlUrl="https://feeds.bbci.co.uk/news/world/rss.xml" htmlUrl="https://bbc.co.uk"/></outline></outline>
            <outline text="Unsafe" xmlUrl="file:///etc/passwd"/><outline text="Dup" xmlUrl="https://feeds.bbci.co.uk/news/world/rss.xml"/>
            <outline text="Loose" type="rss" xmlUrl="https://example.org/feed"></outline><outline text="After" xmlUrl="https://example.org/after"/></body></opml>"#;
        let subs = import(text).unwrap();
        assert_eq!(subs.iter().map(|s| (s.title.as_str(), s.folder.as_str())).collect::<Vec<_>>(), [("BBC", "News / World"), ("Loose", ""), ("After", "")]);
        assert!(import("<rss/>").is_err());
        assert!(import("<opml><body></body></opml>").is_err());
    }
}
