//! A small, tolerant reader for the HTML inside feeds (and the XML of OPML
//! files and discovery pages). Nothing here runs a script or loads a URL:
//! markup becomes plain text or a short list of reader blocks, and every
//! link and image address is resolved and checked before it is kept.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::web_url;

/// One piece of markup. Text is already entity-decoded.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Token<'a> {
    Text(Cow<'a, str>),
    /// Element and attribute names are lowercase.
    Start { name: String, attrs: Vec<(String, String)>, closed: bool },
    End(String),
}

impl Token<'_> {
    pub(super) fn attr<'s>(&'s self, key: &str) -> Option<&'s str> {
        match self {
            Token::Start { attrs, .. } => attrs.iter().find(|(name, _)| name == key).map(|(_, value)| value.as_str()),
            _ => None,
        }
    }
}

/// Elements whose content is never markup or text worth reading.
const RAW_TEXT: [&str; 4] = ["script", "style", "textarea", "xmp"];

/// Tokens of `src`, in order. Comments, doctypes and processing
/// instructions disappear; CDATA sections become text.
pub(super) struct Tokens<'a> {
    src: &'a str,
    pos: usize,
    /// Inside a script or style element: skip to its end tag.
    raw: Option<&'static str>,
}

impl<'a> Tokens<'a> {
    pub(super) fn new(src: &'a str) -> Self { Self { src, pos: 0, raw: None } }

    fn rest(&self) -> &'a str { &self.src[self.pos..] }

    /// Skip past `end`, or to the end of the input.
    fn skip_past(&mut self, end: &str) {
        self.pos = self.rest().find(end).map_or(self.src.len(), |at| self.pos + at + end.len());
    }

    fn tag(&mut self) -> Option<Token<'a>> {
        let rest = self.rest();
        let bytes = rest.as_bytes();
        let closing = bytes.get(1) == Some(&b'/');
        let start = if closing { 2 } else { 1 };
        let name_len = rest[start..].bytes().take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'-' | b'_')).count();
        if name_len == 0 || !bytes[start].is_ascii_alphabetic() { return None; }
        let name = rest[start..start + name_len].to_ascii_lowercase();
        self.pos += start + name_len;
        if closing {
            self.skip_past(">");
            return Some(Token::End(name));
        }
        let mut attrs = Vec::new();
        let mut closed = false;
        loop {
            let rest = self.rest();
            let trimmed = rest.trim_start();
            self.pos += rest.len() - trimmed.len();
            match trimmed.as_bytes().first() {
                None => break,
                Some(b'>') => { self.pos += 1; break; }
                Some(b'/') => {
                    self.pos += 1;
                    if self.rest().starts_with('>') { closed = true; self.pos += 1; break; }
                    continue;
                }
                _ => {}
            }
            let key_len = trimmed.bytes().take_while(|b| !b.is_ascii_whitespace() && !matches!(b, b'=' | b'>' | b'/')).count().max(1);
            let key = trimmed[..key_len].to_ascii_lowercase();
            self.pos += key_len;
            let after = self.rest().trim_start();
            let value = if let Some(after) = after.strip_prefix('=') {
                let after = after.trim_start();
                self.pos = self.src.len() - after.len();
                match after.as_bytes().first() {
                    Some(&quote @ (b'"' | b'\'')) => {
                        let end = after[1..].find(quote as char).map_or(after.len(), |at| at + 1);
                        let value = decode(&after[1..end]).into_owned();
                        self.pos += (end + 1).min(after.len());
                        value
                    }
                    _ => {
                        let end = after.bytes().take_while(|b| !b.is_ascii_whitespace() && *b != b'>').count();
                        self.pos += end;
                        decode(&after[..end]).into_owned()
                    }
                }
            } else { String::new() };
            if attrs.len() < MAX_ATTRS { attrs.push((key, value)); }
        }
        if !closed && let Some(raw) = RAW_TEXT.iter().find(|raw| **raw == name) { self.raw = Some(raw); }
        Some(Token::Start { name, attrs, closed })
    }
}

/// Elements keep at most this many attributes (the rest are skipped).
const MAX_ATTRS: usize = 24;

impl<'a> Iterator for Tokens<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        loop {
            if self.pos >= self.src.len() { return None; }
            if let Some(raw) = self.raw.take() {
                // Skip to `</script`, case-insensitively.
                let rest = self.rest();
                let at = rest.as_bytes().windows(raw.len() + 2).position(|w| w[0] == b'<' && w[1] == b'/' && w[2..].eq_ignore_ascii_case(raw.as_bytes()));
                self.pos = at.map_or(self.src.len(), |at| self.pos + at);
                continue;
            }
            let rest = self.rest();
            if !rest.starts_with('<') {
                let end = rest.find('<').unwrap_or(rest.len());
                self.pos += end;
                return Some(Token::Text(decode(&rest[..end])));
            }
            if rest.starts_with("<!--") { self.skip_past("-->"); continue; }
            if let Some(body) = rest.strip_prefix("<![CDATA[") {
                let end = body.find("]]>").unwrap_or(body.len());
                self.pos += "<![CDATA[".len() + end + "]]>".len().min(body.len() - end);
                return Some(Token::Text(Cow::Borrowed(&body[..end])));
            }
            if rest.starts_with("<!") || rest.starts_with("<?") { self.skip_past(">"); continue; }
            if let Some(tag) = self.tag() { return Some(tag); }
            // A lone `<` is text.
            self.pos += 1;
            return Some(Token::Text(Cow::Borrowed("<")));
        }
    }
}

/// Decode character references. Unknown names stay as written.
pub(super) fn decode(text: &str) -> Cow<'_, str> {
    if !text.contains('&') { return Cow::Borrowed(text); }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        match reference(rest) {
            Some((c, len)) => { out.push(c); rest = &rest[len..]; }
            None => { out.push('&'); rest = &rest[1..]; }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// The character a reference at the start of `s` (`&…`) stands for, and
/// its length. A few common names work without their semicolon.
fn reference(s: &str) -> Option<(char, usize)> {
    let body = &s[1..];
    if let Some(num) = body.strip_prefix('#') {
        let (digits, radix, skip) = match num.strip_prefix(['x', 'X']) { Some(hex) => (hex, 16, 3), None => (num, 10, 2) };
        let len = digits.bytes().take_while(|b| (*b as char).is_digit(radix)).count();
        if len == 0 || len > 8 { return None; }
        let code = u32::from_str_radix(&digits[..len], radix).ok()?;
        let c = match code { 0 => '\u{FFFD}', 0x80..=0x9F => windows_1252(code), _ => char::from_u32(code).unwrap_or('\u{FFFD}') };
        let semi = usize::from(digits.as_bytes().get(len) == Some(&b';'));
        return Some((c, skip + len + semi));
    }
    let len = body.bytes().take_while(u8::is_ascii_alphanumeric).count();
    if len == 0 || len > 12 { return None; }
    let name = &body[..len];
    let semi = body.as_bytes().get(len) == Some(&b';');
    let c = named(name)?;
    if !semi && !matches!(name, "amp" | "lt" | "gt" | "quot" | "nbsp") { return None; }
    Some((c, 1 + len + usize::from(semi)))
}

/// Numeric references in 0x80–0x9F mean Windows-1252 in practice.
fn windows_1252(code: u32) -> char {
    const TABLE: [char; 32] = ['€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž', '\u{8F}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}', 'ž', 'Ÿ'];
    TABLE[(code - 0x80) as usize]
}

/// The named references feeds actually use (HTML's full table is ~2,000).
fn named(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&', "lt" => '<', "gt" => '>', "quot" => '"', "apos" => '\'', "nbsp" => '\u{a0}',
        "ndash" => '–', "mdash" => '—', "minus" => '−', "hellip" => '…', "bull" => '•', "middot" => '·',
        "lsquo" => '‘', "rsquo" => '’', "sbquo" => '‚', "ldquo" => '“', "rdquo" => '”', "bdquo" => '„',
        "laquo" => '«', "raquo" => '»', "lsaquo" => '‹', "rsaquo" => '›', "prime" => '′', "Prime" => '″',
        "copy" => '©', "reg" => '®', "trade" => '™', "deg" => '°', "times" => '×', "divide" => '÷',
        "plusmn" => '±', "micro" => 'µ', "para" => '¶', "sect" => '§', "dagger" => '†', "Dagger" => '‡',
        "euro" => '€', "pound" => '£', "yen" => '¥', "cent" => '¢', "curren" => '¤', "permil" => '‰',
        "frac12" => '½', "frac14" => '¼', "frac34" => '¾', "sup1" => '¹', "sup2" => '²', "sup3" => '³',
        "iexcl" => '¡', "iquest" => '¿', "shy" => '\u{ad}', "zwj" => '\u{200d}', "zwnj" => '\u{200c}',
        "lrm" => '\u{200e}', "rlm" => '\u{200f}', "thinsp" => '\u{2009}', "ensp" => '\u{2002}', "emsp" => '\u{2003}',
        "larr" => '←', "rarr" => '→', "uarr" => '↑', "darr" => '↓', "harr" => '↔', "hearts" => '♥',
        "acute" => '´', "cedil" => '¸', "uml" => '¨', "macr" => '¯', "ordf" => 'ª', "ordm" => 'º', "not" => '¬',
        "Agrave" => 'À', "Aacute" => 'Á', "Acirc" => 'Â', "Atilde" => 'Ã', "Auml" => 'Ä', "Aring" => 'Å', "AElig" => 'Æ',
        "Ccedil" => 'Ç', "Egrave" => 'È', "Eacute" => 'É', "Ecirc" => 'Ê', "Euml" => 'Ë', "Igrave" => 'Ì', "Iacute" => 'Í',
        "Icirc" => 'Î', "Iuml" => 'Ï', "ETH" => 'Ð', "Ntilde" => 'Ñ', "Ograve" => 'Ò', "Oacute" => 'Ó', "Ocirc" => 'Ô',
        "Otilde" => 'Õ', "Ouml" => 'Ö', "Oslash" => 'Ø', "Ugrave" => 'Ù', "Uacute" => 'Ú', "Ucirc" => 'Û', "Uuml" => 'Ü',
        "Yacute" => 'Ý', "THORN" => 'Þ', "szlig" => 'ß', "agrave" => 'à', "aacute" => 'á', "acirc" => 'â', "atilde" => 'ã',
        "auml" => 'ä', "aring" => 'å', "aelig" => 'æ', "ccedil" => 'ç', "egrave" => 'è', "eacute" => 'é', "ecirc" => 'ê',
        "euml" => 'ë', "igrave" => 'ì', "iacute" => 'í', "icirc" => 'î', "iuml" => 'ï', "eth" => 'ð', "ntilde" => 'ñ',
        "ograve" => 'ò', "oacute" => 'ó', "ocirc" => 'ô', "otilde" => 'õ', "ouml" => 'ö', "oslash" => 'ø', "ugrave" => 'ù',
        "uacute" => 'ú', "ucirc" => 'û', "uuml" => 'ü', "yacute" => 'ý', "thorn" => 'þ', "yuml" => 'ÿ', "OElig" => 'Œ',
        "oelig" => 'œ', "Scaron" => 'Š', "scaron" => 'š', "Yuml" => 'Ÿ', "fnof" => 'ƒ', "circ" => 'ˆ', "tilde" => '˜',
        _ => return None,
    })
}

/// Collapse runs of whitespace to one space (no leading or trailing space).
fn push_collapsed(out: &mut String, text: &str, space_pending: &mut bool) {
    for c in text.chars() {
        if c.is_whitespace() && c != '\u{a0}' {
            *space_pending = !out.is_empty();
        } else {
            if *space_pending { out.push(' '); *space_pending = false; }
            out.push(if c == '\u{a0}' { ' ' } else { c });
        }
    }
}

/// Plain text of markup, entities decoded and whitespace collapsed, at
/// most `max_chars` characters (an ellipsis marks a cut).
pub fn plain_text(html: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut space = false;
    let mut skip = Skip::default();
    for token in Tokens::new(html) {
        match &token {
            Token::Start { name, closed, .. } => {
                if skip.start(name, *closed) { continue; }
                if is_block(name) || name == "br" { space = !out.is_empty(); }
            }
            Token::End(name) => {
                if skip.end(name) { continue; }
                if is_block(name) { space = !out.is_empty(); }
            }
            Token::Text(text) if !skip.active() => push_collapsed(&mut out, text, &mut space),
            Token::Text(_) => {}
        }
        if out.chars().count() > max_chars { break; }
    }
    truncate_chars(out, max_chars)
}

/// Cut `text` to `max` characters on a word boundary near the end, adding
/// an ellipsis when anything was cut.
pub(super) fn truncate_chars(text: String, max: usize) -> String {
    let Some((cut, _)) = text.char_indices().nth(max) else { return text };
    let head = &text[..cut];
    let soft = head.rfind(' ').filter(|at| *at > cut * 3 / 4).unwrap_or(cut);
    let mut out = head[..soft].trim_end().to_owned();
    out.push('…');
    out
}

/// Elements dropped with everything inside them: active content, forms,
/// embedded documents and media, and page chrome.
const DROPPED: [&str; 22] = [
    "script", "style", "iframe", "frame", "frameset", "object", "embed", "applet", "form", "input", "button", "select",
    "textarea", "svg", "math", "canvas", "template", "noscript", "video", "audio", "head", "nav",
];
/// Elements that never have content or an end tag.
const VOID: [&str; 14] = ["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr"];

fn is_block(name: &str) -> bool {
    matches!(name, "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "figure" | "figcaption" | "blockquote" | "ul" | "ol" | "li"
        | "dl" | "dt" | "dd" | "pre" | "hr" | "table" | "tr" | "td" | "th" | "thead" | "tbody" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "aside" | "details" | "summary" | "center" | "address")
}

/// Tracks a dropped element (and nested copies of it) until it ends.
#[derive(Default)]
struct Skip { name: Option<String>, depth: u32 }

impl Skip {
    fn active(&self) -> bool { self.name.is_some() }

    /// Returns whether the token is skipped.
    fn start(&mut self, name: &str, closed: bool) -> bool {
        match &self.name {
            Some(skipped) => { if skipped == name && !closed { self.depth += 1; } true }
            None if DROPPED.contains(&name) => {
                if !closed && !VOID.contains(&name) { self.name = Some(name.to_owned()); self.depth = 1; }
                true
            }
            None => false,
        }
    }

    fn end(&mut self, name: &str) -> bool {
        let Some(skipped) = &self.name else { return false };
        if skipped == name {
            self.depth -= 1;
            if self.depth == 0 { self.name = None; }
        }
        true
    }
}

// ── Reader blocks ────────────────────────────────────────────────────────

/// Inline style bits of a [`Mark`].
pub const BOLD: u8 = 1;
pub const ITALIC: u8 = 2;
pub const CODE: u8 = 4;
pub const STRIKE: u8 = 8;

/// A styled run of a [`Rich`] text (byte range, style bits, link).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    pub start: u32,
    pub end: u32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub style: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

fn is_zero(v: &u8) -> bool { *v == 0 }

/// Text with styled runs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rich {
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub marks: Vec<Mark>,
}

impl Rich {
    fn push(&mut self, text: &str, style: u8, link: Option<&str>) {
        if text.is_empty() { return; }
        let start = self.text.len() as u32;
        self.text.push_str(text);
        let end = self.text.len() as u32;
        if style == 0 && link.is_none() { return; }
        if let Some(last) = self.marks.last_mut() && last.end == start && last.style == style && last.link.as_deref() == link {
            last.end = end;
            return;
        }
        self.marks.push(Mark { start, end, style, link: link.map(str::to_owned) });
    }

    /// Drop a trailing space and runs that point past the text.
    fn finish(mut self) -> Option<Self> {
        let trimmed = self.text.trim_end().len();
        self.text.truncate(trimmed);
        let len = self.text.len() as u32;
        self.marks.retain_mut(|mark| { mark.end = mark.end.min(len); mark.start < mark.end });
        (!self.text.trim().is_empty()).then_some(self)
    }
}

/// One block of a cleaned-up article.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Block {
    Heading { level: u8, text: Rich },
    Paragraph(Rich),
    Quote(Rich),
    /// A list item: its number in an ordered list, and its nesting depth.
    Item { number: Option<u32>, depth: u8, text: Rich },
    Code(String),
    Image { src: String, alt: String },
    Rule,
}

/// An article's content as reader blocks: no markup, scripts, frames,
/// forms or tracking pixels, and only web links.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Article {
    pub blocks: Vec<Block>,
}

/// Article text kept at most (bytes); the rest is a click away online.
pub const MAX_ARTICLE_TEXT: usize = 48 * 1024;
const MAX_BLOCKS: usize = 600;
const MAX_IMAGES: usize = 24;
/// A link or image address longer than this is dropped.
const MAX_URL_BYTES: usize = 4096;

impl Article {
    /// Approximate bytes this article takes in memory and on disk.
    pub fn weight(&self) -> usize {
        self.blocks.iter().map(|block| match block {
            Block::Heading { text, .. } | Block::Paragraph(text) | Block::Quote(text) | Block::Item { text, .. } => text.text.len() + text.marks.len() * 24,
            Block::Code(text) => text.len(),
            Block::Image { src, alt } => src.len() + alt.len(),
            Block::Rule => 1,
        } + 8).sum()
    }

    /// The article's image addresses, in order.
    pub fn images(&self) -> impl Iterator<Item = &str> {
        self.blocks.iter().filter_map(|block| match block { Block::Image { src, .. } => Some(src.as_str()), _ => None })
    }
}

/// Image addresses that only count readers (pixels, beacons, feed proxies).
const TRACKERS: [&str; 16] = [
    "feeds.feedburner.com/~r/", "feeds.feedburner.com/~ff/", "feedproxy.google.com/~r/", "pixel.wp.com", "stats.wordpress.com",
    "doubleclick.net", "google-analytics.com", "googletagmanager.com", "pi.feedsportal.com", "feedsportal.com/c/",
    "/tracking/", "/beacon", "/pixel.gif", "/pixel.png", "count.gif", "analytics",
];

/// Whether an image address is a tracker rather than a picture.
pub fn is_tracker(src: &str) -> bool {
    let lower = src.to_ascii_lowercase();
    TRACKERS.iter().any(|pattern| lower.contains(pattern))
}

/// Resolve `href` against `base`, keeping only web addresses.
pub(super) fn resolve(base: Option<&url::Url>, href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() || href.len() > MAX_URL_BYTES || href.starts_with('#') { return None; }
    let url = match base { Some(base) => base.join(href).ok()?, None => url::Url::parse(href).ok()? };
    let url = String::from(url);
    web_url(&url).then_some(url)
}

/// The first usable image address of an `<img>`: `src`, a lazy-loading
/// attribute, or the first `srcset` candidate.
fn image_source<'t>(token: &'t Token<'_>) -> Option<&'t str> {
    ["src", "data-src", "data-lazy-src", "data-original"].iter().filter_map(|key| token.attr(key)).find(|src| !src.trim().is_empty() && !src.trim_start().starts_with("data:"))
        .or_else(|| token.attr("srcset").and_then(|set| set.split(',').next()).and_then(|first| first.split_whitespace().next()))
}

/// A tracking pixel declares itself by its size.
fn tiny(token: &Token<'_>) -> bool {
    ["width", "height"].iter().any(|key| token.attr(key).and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok()).is_some_and(|v| v <= 2.0))
}

/// What a text run currently flushes into.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind { Paragraph, Heading(u8), Item(Option<u32>, u8) }

struct Builder<'b> {
    base: Option<&'b url::Url>,
    blocks: Vec<Block>,
    rich: Rich,
    kind: Kind,
    space: bool,
    bold: u32,
    italic: u32,
    code: u32,
    strike: u32,
    links: Vec<Option<String>>,
    quote: u32,
    pre: Option<String>,
    lists: Vec<(bool, u32)>,
    text_bytes: usize,
    images: usize,
    full: bool,
}

impl Builder<'_> {
    fn style(&self) -> u8 {
        (if self.bold > 0 { BOLD } else { 0 }) | (if self.italic > 0 { ITALIC } else { 0 }) | (if self.code > 0 { CODE } else { 0 }) | (if self.strike > 0 { STRIKE } else { 0 })
    }

    fn push_block(&mut self, block: Block) {
        if self.blocks.len() >= MAX_BLOCKS { self.full = true; return; }
        self.blocks.push(block);
    }

    fn flush(&mut self) {
        self.space = false;
        let Some(rich) = std::mem::take(&mut self.rich).finish() else { return };
        let block = match self.kind {
            Kind::Heading(level) => Block::Heading { level, text: rich },
            Kind::Item(number, depth) => Block::Item { number, depth, text: rich },
            Kind::Paragraph if self.quote > 0 => Block::Quote(rich),
            Kind::Paragraph => Block::Paragraph(rich),
        };
        self.push_block(block);
        if let Kind::Item(..) = self.kind { self.kind = Kind::Paragraph; }
    }

    fn text(&mut self, text: &str) {
        if self.full { return; }
        let room = MAX_ARTICLE_TEXT.saturating_sub(self.text_bytes);
        let text = if text.len() > room { self.full = true; &text[..text.floor_char_boundary(room)] } else { text };
        if let Some(pre) = &mut self.pre {
            pre.push_str(text);
            self.text_bytes += text.len();
        } else {
            let style = self.style();
            let link = self.links.iter().rev().find_map(Option::as_deref).map(str::to_owned);
            let mut out = String::with_capacity(text.len());
            for c in text.chars() {
                if c.is_whitespace() && c != '\u{a0}' { self.space = true; continue; }
                if self.space {
                    self.space = false;
                    if !out.is_empty() {
                        out.push(' ');
                    } else if !self.rich.text.is_empty() && !self.rich.text.ends_with('\n') {
                        // The space between two runs stays unstyled.
                        self.rich.push(" ", 0, None);
                    }
                }
                out.push(if c == '\u{a0}' { ' ' } else { c });
            }
            self.text_bytes += out.len();
            self.rich.push(&out, style, link.as_deref());
        }
        if self.text_bytes > MAX_ARTICLE_TEXT { self.full = true; }
    }

    fn start(&mut self, token: &Token<'_>, name: &str, closed: bool) {
        match name {
            "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "figure" | "figcaption" | "dl" | "dt" | "dd" | "table" | "aside" | "details" | "summary" | "center" | "address" => self.flush(),
            "tr" => self.flush(),
            "td" | "th" => if !self.rich.text.is_empty() { self.rich.push(" · ", 0, None); },
            "br" => if let Some(pre) = &mut self.pre { pre.push('\n'); } else if !self.rich.text.is_empty() { self.rich.push("\n", 0, None); self.space = false; },
            "hr" => { self.flush(); self.push_block(Block::Rule); }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => { self.flush(); self.kind = Kind::Heading(name.as_bytes()[1] - b'0'); }
            "blockquote" => { self.flush(); self.quote += 1; }
            "ul" | "ol" => { self.flush(); self.lists.push((name == "ol", token.attr("start").and_then(|v| v.parse().ok()).unwrap_or(1))); }
            "li" => {
                self.flush();
                let depth = self.lists.len().saturating_sub(1).min(4) as u8;
                let number = match self.lists.last_mut() {
                    Some((true, next)) => { let n = *next; *next += 1; Some(n) }
                    _ => None,
                };
                self.kind = Kind::Item(number, depth);
            }
            "pre" => { self.flush(); self.pre = Some(String::new()); }
            "b" | "strong" => self.bold += 1,
            "i" | "em" | "cite" | "dfn" => self.italic += 1,
            "code" | "kbd" | "samp" | "tt" | "var" => self.code += 1,
            "s" | "del" | "strike" => self.strike += 1,
            "a" if !closed => {
                let href = token.attr("href").and_then(|href| resolve(self.base, href));
                self.links.push(href);
            }
            "img" => {
                if self.images >= MAX_IMAGES || tiny(token) { return; }
                let Some(src) = image_source(token).and_then(|src| resolve(self.base, src)).filter(|src| !is_tracker(src)) else { return };
                let alt = token.attr("alt").map(|alt| plain_text(alt, 200)).unwrap_or_default();
                self.flush();
                self.images += 1;
                self.text_bytes += src.len() + alt.len();
                self.push_block(Block::Image { src, alt });
            }
            _ => {}
        }
    }

    fn end(&mut self, name: &str) {
        match name {
            "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "figure" | "figcaption" | "dl" | "dt" | "dd" | "table" | "tr" | "aside" | "details" | "summary" | "center" | "address" | "li" => self.flush(),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => { self.flush(); self.kind = Kind::Paragraph; }
            "blockquote" => { self.flush(); self.quote = self.quote.saturating_sub(1); }
            "ul" | "ol" => { self.flush(); self.lists.pop(); self.kind = Kind::Paragraph; }
            "pre" => {
                if let Some(code) = self.pre.take() {
                    let code = code.trim_matches('\n').trim_end().to_owned();
                    if !code.trim().is_empty() { self.push_block(Block::Code(code)); }
                }
            }
            "b" | "strong" => self.bold = self.bold.saturating_sub(1),
            "i" | "em" | "cite" | "dfn" => self.italic = self.italic.saturating_sub(1),
            "code" | "kbd" | "samp" | "tt" | "var" => self.code = self.code.saturating_sub(1),
            "s" | "del" | "strike" => self.strike = self.strike.saturating_sub(1),
            "a" => { self.links.pop(); }
            _ => {}
        }
    }
}

/// Clean up an article's HTML into reader blocks. Relative links and
/// images resolve against `base` (the article's address).
pub fn sanitize(html: &str, base: Option<&str>) -> Article {
    let base = base.and_then(|base| url::Url::parse(base).ok());
    let mut b = Builder {
        base: base.as_ref(), blocks: Vec::new(), rich: Rich::default(), kind: Kind::Paragraph, space: false, bold: 0, italic: 0, code: 0, strike: 0,
        links: Vec::new(), quote: 0, pre: None, lists: Vec::new(), text_bytes: 0, images: 0, full: false,
    };
    let mut skip = Skip::default();
    for token in Tokens::new(html) {
        if b.full { break; }
        match &token {
            Token::Start { name, closed, .. } => {
                if skip.start(name, *closed) { continue; }
                b.start(&token, name, *closed);
            }
            Token::End(name) => {
                if skip.end(name) { continue; }
                b.end(name);
            }
            Token::Text(text) if !skip.active() => b.text(text),
            Token::Text(_) => {}
        }
    }
    if let Some(code) = b.pre.take() && !code.trim().is_empty() { b.push_block(Block::Code(code.trim_end().to_owned())); }
    b.flush();
    Article { blocks: b.blocks }
}

/// Plain text as reader paragraphs (blank lines separate them).
pub fn paragraphs(text: &str) -> Article {
    let mut blocks = Vec::new();
    let mut bytes = 0;
    for chunk in text.split("\n\n") {
        let mut rich = Rich::default();
        let mut space = false;
        let mut collapsed = String::new();
        push_collapsed(&mut collapsed, chunk, &mut space);
        bytes += collapsed.len();
        rich.push(&collapsed, 0, None);
        if let Some(rich) = rich.finish() { blocks.push(Block::Paragraph(rich)); }
        if bytes > MAX_ARTICLE_TEXT || blocks.len() >= MAX_BLOCKS { break; }
    }
    Article { blocks }
}

/// The first picture in markup that is not a tracker, resolved.
pub(super) fn first_image(html: &str, base: Option<&url::Url>) -> Option<String> {
    let mut skip = Skip::default();
    for token in Tokens::new(html) {
        match &token {
            Token::Start { name, closed, .. } => {
                if skip.start(name, *closed) { continue; }
                if name == "img" && !tiny(&token) && let Some(src) = image_source(&token).and_then(|src| resolve(base, src)).filter(|src| !is_tracker(src)) {
                    return Some(src);
                }
            }
            Token::End(name) => { skip.end(name); }
            Token::Text(_) => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(article: &Article) -> Vec<String> {
        article.blocks.iter().map(|block| match block {
            Block::Heading { level, text } => format!("h{level}:{}", text.text),
            Block::Paragraph(text) => format!("p:{}", text.text),
            Block::Quote(text) => format!("q:{}", text.text),
            Block::Item { number, depth, text } => format!("li{}{}:{}", number.map(|n| n.to_string()).unwrap_or_default(), depth, text.text),
            Block::Code(code) => format!("code:{code}"),
            Block::Image { src, alt } => format!("img:{src}|{alt}"),
            Block::Rule => "hr".into(),
        }).collect()
    }

    #[test]
    fn entities_decode_named_numeric_and_legacy_references() {
        assert_eq!(decode("Tom &amp; Jerry &mdash; &#8220;hi&#x201D; &copy;2026 &nbsp;&unknown; AT&T"), "Tom & Jerry — “hi” ©2026 \u{a0}&unknown; AT&T");
        assert_eq!(decode("&#150; &#0; &#x110000;"), "– \u{FFFD} \u{FFFD}");
        assert_eq!(decode("&amp &lt;3 &ampx"), "& <3 &ampx");
        assert_eq!(decode("plain"), "plain");
    }

    #[test]
    fn plain_text_strips_markup_scripts_and_collapses_space() {
        let html = "<p>Hello <b>wide</b>\n\n world</p><script>alert('x')</script><style>p{}</style><p>Second&nbsp;line</p><!-- note --><iframe src=x>frame text</iframe>";
        assert_eq!(plain_text(html, 200), "Hello wide world Second line");
        let long = plain_text(&"word ".repeat(100), 20);
        assert!(long.ends_with('…') && long.chars().count() <= 21, "{long}");
        assert_eq!(plain_text("<![CDATA[<b>cdata</b>]]>", 50), "<b>cdata</b>");
    }

    #[test]
    fn sanitizing_keeps_structure_and_drops_active_content() {
        let html = r#"<h2>Title</h2><p>Some <strong>bold</strong> and <em>italic</em> <a href="/rel">link</a> <a href="javascript:alert(1)">bad</a>.</p>
            <script>document.cookie</script><iframe src="https://evil.example/"></iframe><form><input value="x"></form>
            <svg><text>vector</text></svg><object data="x.swf">flash</object>
            <blockquote><p>Quoted</p></blockquote><ul><li>one</li><li>two<ol><li>nested</li></ol></li></ul>
            <pre><code>let x = 1;
let y = 2;</code></pre><hr><img src="pic.jpg" alt="A &amp; B"><img src="https://feeds.feedburner.com/~r/x/~4/abc" ><img src="https://example.org/p.gif" width="1" height="1">
            <img src="data:image/png;base64,AAAA"><p onclick="steal()" style="color:red">Attrs gone</p>"#;
        let article = sanitize(html, Some("https://example.org/news/post"));
        assert_eq!(texts(&article), [
            "h2:Title", "p:Some bold and italic link bad.", "q:Quoted", "li0:one", "li0:two", "li11:nested",
            "code:let x = 1;\nlet y = 2;", "hr", "img:https://example.org/news/pic.jpg|A & B", "p:Attrs gone",
        ]);
        let Block::Paragraph(rich) = &article.blocks[1] else { panic!() };
        let mark = |s: &str| rich.marks.iter().find(|m| &rich.text[m.start as usize..m.end as usize] == s).cloned();
        assert_eq!(mark("bold").unwrap().style, BOLD);
        assert_eq!(mark("italic").unwrap().style, ITALIC);
        assert_eq!(mark("link").unwrap().link.as_deref(), Some("https://example.org/rel"));
        assert!(mark("bad").is_none(), "script links are plain text");
        let all = texts(&article).join(" ");
        for banned in ["cookie", "evil", "vector", "flash", "steal", "feedburner", "p.gif", "data:"] { assert!(!all.contains(banned), "{banned} in {all}"); }
    }

    #[test]
    fn sanitizing_is_bounded() {
        let html = "<p>".to_owned() + &"lorem ipsum ".repeat(20_000) + "</p>" + &"<img src=\"https://example.org/i.png\">".repeat(100);
        let article = sanitize(&html, None);
        assert!(article.weight() <= MAX_ARTICLE_TEXT + 1024, "{}", article.weight());
        assert!(sanitize(&"<p>x</p>".repeat(5000), None).blocks.len() <= MAX_BLOCKS);
        assert!(sanitize(&"<img src=\"https://example.org/i.png\">".repeat(100), None).images().count() <= MAX_IMAGES);
    }

    #[test]
    fn malformed_markup_never_panics() {
        for html in ["<", "<a", "<a href=\"x", "</", "<!--", "<![CDATA[", "<p><b>unclosed", "&#", "&#x;", "<img src=>", "<<>>", "<ul><li>x", "a<b>c</b", "<pre>x"] {
            let _ = sanitize(html, Some("https://example.org/"));
            let _ = plain_text(html, 10);
        }
        assert_eq!(texts(&sanitize("<ul><li>x", None)), ["li0:x"]);
    }

    #[test]
    fn first_image_skips_pixels_and_resolves() {
        let html = r#"<img src="https://x.example/pixel.gif" width=1 height=1><img data-src="/hero.webp" src="data:image/gif;base64,R0">"#;
        let base = url::Url::parse("https://example.org/a/b").unwrap();
        assert_eq!(first_image(html, Some(&base)).as_deref(), Some("https://example.org/hero.webp"));
        assert_eq!(first_image("<p>none</p>", Some(&base)), None);
    }
}
