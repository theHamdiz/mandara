//! Reddit Markdown into the shell's own text elements. Parsing runs on the
//! worker, once per post or comment; the source text is dropped after.
//! Links are kept only when they are web addresses (Reddit's `/r/` and
//! `/u/` links become reddit.com addresses), raw HTML stays plain text,
//! and `>!spoilers!<` are masked until the reader reveals them.

use crate::markdown::{Block, Doc, Inline};

/// What a masked spoiler shows, sized to the hidden text up to this many marks.
const MASK: char = '▇';
const MAX_MASK: usize = 12;

/// A parsed body, plus the unmasked version when it has spoilers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Body {
    pub doc: Doc,
    pub revealed: Option<Box<Doc>>,
}

impl Body {
    pub fn parse(markdown: &str) -> Self {
        let masked = mask_spoilers(markdown);
        match masked {
            Some(masked) => Self { doc: sanitized(&masked), revealed: Some(Box::new(sanitized(&unmark_spoilers(markdown)))) },
            None => Self { doc: sanitized(markdown), revealed: None },
        }
    }

    /// The document to draw: the revealed one when asked for and present.
    pub fn shown(&self, reveal: bool) -> &Doc {
        match (&self.revealed, reveal) { (Some(doc), true) => doc, _ => &self.doc }
    }

    pub fn has_spoilers(&self) -> bool { self.revealed.is_some() }

    pub fn is_empty(&self) -> bool { self.doc.blocks.is_empty() }
}

/// Spoiler ranges `>!…!<` on one line, as byte ranges including the marks.
fn spoilers(text: &str) -> impl Iterator<Item = std::ops::Range<usize>> + '_ {
    let mut at = 0;
    std::iter::from_fn(move || {
        let start = at + text.get(at..)?.find(">!")?;
        let end = start + 2 + text[start + 2..].find("!<")?;
        if text[start..end].contains('\n') {
            at = start + 2;
            return Some(start..start);
        }
        at = end + 2;
        Some(start..end + 2)
    }).filter(|range| !range.is_empty())
}

/// The text with each spoiler replaced by marks, or `None` without spoilers.
fn mask_spoilers(text: &str) -> Option<String> {
    let mut out = String::new();
    let mut last = 0;
    for range in spoilers(text) {
        out.push_str(&text[last..range.start]);
        let hidden = text[range.start + 2..range.end - 2].chars().count().clamp(1, MAX_MASK);
        out.extend(std::iter::repeat_n(MASK, hidden));
        last = range.end;
    }
    if last == 0 { return None; }
    out.push_str(&text[last..]);
    Some(out)
}

/// The text with spoiler marks removed.
fn unmark_spoilers(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for range in spoilers(text) {
        out.push_str(&text[last..range.start]);
        out.push_str(&text[range.start + 2..range.end - 2]);
        last = range.end;
    }
    out.push_str(&text[last..]);
    out
}

/// Parse and keep only safe links.
fn sanitized(markdown: &str) -> Doc {
    let mut doc = crate::markdown::parse(markdown);
    for block in &mut doc.blocks { clean_block(block); }
    doc
}

fn clean_block(block: &mut Block) {
    match block {
        Block::Heading { text, .. } | Block::Paragraph(text) => clean_inline(text),
        Block::Quote(blocks) => blocks.iter_mut().for_each(clean_block),
        Block::List { items, .. } => items.iter_mut().flat_map(|item| item.blocks.iter_mut()).for_each(clean_block),
        Block::Table { head, rows } => head.iter_mut().chain(rows.iter_mut().flatten()).for_each(clean_inline),
        Block::Code { .. } | Block::Rule => {}
    }
}

fn clean_inline(inline: &mut Inline) {
    for span in &mut inline.spans {
        span.link = span.link.take().and_then(|link| safe_link(&link));
    }
}

/// A web address to open, or `None` for anything else.
pub(super) fn safe_link(link: &str) -> Option<String> {
    let link = link.trim();
    if link.starts_with("/r/") || link.starts_with("/u/") || link.starts_with("/user/") {
        return link[1..].bytes().all(|b| b.is_ascii_graphic()).then(|| format!("https://www.reddit.com{link}"));
    }
    if link.starts_with("r/") || link.starts_with("u/") {
        return link.bytes().all(|b| b.is_ascii_graphic()).then(|| format!("https://www.reddit.com/{link}"));
    }
    mandara_services::feeds::web_url(link).then(|| link.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links(doc: &Doc) -> Vec<Option<String>> {
        let mut out = Vec::new();
        fn walk(blocks: &[Block], out: &mut Vec<Option<String>>) {
            for block in blocks {
                match block {
                    Block::Paragraph(text) | Block::Heading { text, .. } => out.extend(text.spans.iter().filter(|span| span.style.bold || span.link.is_some() || text.text[span.range.clone()].contains("link")).map(|span| span.link.clone())),
                    Block::Quote(inner) => walk(inner, out),
                    Block::List { items, .. } => items.iter().for_each(|item| walk(&item.blocks, out)),
                    _ => {}
                }
            }
        }
        walk(&doc.blocks, &mut out);
        out
    }

    #[test]
    fn only_web_and_reddit_links_survive_and_html_stays_text() {
        let body = Body::parse("[web](https://example.org) [sub](/r/rust) [user](u/someone) [script](javascript:alert(1)) [file](file:///etc/passwd) [cred](https://user:pw@example.org)\n\n<script>alert(1)</script> <img src=x onerror=y>");
        let found: Vec<Option<String>> = match &body.doc.blocks[0] { Block::Paragraph(text) => text.spans.iter().map(|span| span.link.clone()).collect(), other => panic!("{other:?}") };
        assert!(found.contains(&Some("https://example.org".into())));
        assert!(found.contains(&Some("https://www.reddit.com/r/rust".into())));
        assert!(found.contains(&Some("https://www.reddit.com/u/someone".into())));
        assert!(!found.iter().flatten().any(|link| link.starts_with("javascript") || link.starts_with("file") || link.contains('@')), "{found:?}");
        let mut plain = String::new();
        crate::markdown::plain(&body.doc.blocks, &mut plain);
        assert!(plain.contains("<script>alert(1)</script>"), "raw HTML is shown as text, never run: {plain}");
        assert!(links(&body.doc).iter().flatten().all(|link| link.starts_with("https://")));
    }

    #[test]
    fn spoilers_are_masked_until_revealed() {
        let body = Body::parse("The ending: >!he was a ghost!< and >!more!<, but >!not\nacross lines!< here.");
        assert!(body.has_spoilers());
        let mut masked = String::new();
        crate::markdown::plain(&body.shown(false).blocks, &mut masked);
        assert!(!masked.contains("ghost") && masked.contains('▇'), "{masked}");
        assert!(masked.contains(">!not"), "a spoiler never spans lines: {masked}");
        let mut revealed = String::new();
        crate::markdown::plain(&body.shown(true).blocks, &mut revealed);
        assert!(revealed.contains("he was a ghost") && !revealed.contains(">!he"), "{revealed}");
        let plain = Body::parse("No secrets **here**");
        assert!(!plain.has_spoilers());
        assert_eq!(plain.shown(true), &plain.doc);
    }
}
