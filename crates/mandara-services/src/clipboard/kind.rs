//! What a clipboard entry holds, decided once when it is copied (on the
//! watcher thread, never the UI thread) and stored with the entry: plain
//! text, a link, Markdown, a code snippet with its best-guess language, CSS,
//! a colour value, an image (screenshots told apart) or files.
//!
//! Every check is pure and cheap: only the first [`SCAN_BYTES`] of a long
//! copy are looked at, whole-value kinds (colours, links, file lists) need
//! the whole text to be that value, and the order below settles overlaps
//! (a colour beats a link, a lone fenced block is code, Markdown with prose
//! around its code is Markdown).

use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{color, lang};

/// Bytes of a text copy that are inspected (the head of a longer copy
/// decides; a whole value can't be longer than this anyway).
pub const SCAN_BYTES: usize = 8 * 1024;
/// Bytes of a saved image read to learn its size and origin.
const IMAGE_HEAD_BYTES: u64 = 64 * 1024;

/// The kinds the panel can filter by.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    #[default]
    Text,
    Link,
    Markdown,
    Code,
    Css,
    Color,
    Image,
    Files,
}

impl Kind {
    /// Every kind, in the order the panel's chips list them.
    pub const ALL: [Kind; 8] = [Kind::Text, Kind::Link, Kind::Markdown, Kind::Code, Kind::Css, Kind::Color, Kind::Image, Kind::Files];

    /// Stable id (`clipboard kind <id>`, JSON).
    pub fn id(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Link => "link",
            Kind::Markdown => "markdown",
            Kind::Code => "code",
            Kind::Css => "css",
            Kind::Color => "color",
            Kind::Image => "image",
            Kind::Files => "files",
        }
    }

    /// Reads a kind name: ids, plurals and common synonyms.
    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "text" | "texts" | "plain" => Kind::Text,
            "link" | "links" | "url" | "urls" => Kind::Link,
            "markdown" | "md" => Kind::Markdown,
            "code" | "snippet" | "snippets" => Kind::Code,
            "css" => Kind::Css,
            "color" | "colors" | "colour" | "colours" => Kind::Color,
            "image" | "images" | "screenshot" | "screenshots" | "picture" | "pictures" => Kind::Image,
            "file" | "files" => Kind::Files,
            _ => return None,
        })
    }
}

/// An entry's kind plus what its preview needs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ClipClass {
    #[default]
    Text,
    Link {
        host: String,
    },
    Markdown,
    Code {
        /// `rust`, `python`, `json`, … or empty when only the shape says code.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        lang: String,
    },
    Css,
    Color {
        /// How it was written: `hex`, `rgb`, `hsl`, `hwb`, `lab`, `lch`,
        /// `oklab`, `oklch` or `hyprland`.
        notation: String,
        /// The swatch, as 8-bit sRGB with alpha.
        rgba: [u8; 4],
    },
    Image {
        #[serde(default)]
        width: u32,
        #[serde(default)]
        height: u32,
        /// Taken by a screenshot tool or exactly a display's size.
        #[serde(default)]
        screenshot: bool,
    },
    Files {
        count: usize,
    },
}

impl ClipClass {
    pub fn kind(&self) -> Kind {
        match self {
            ClipClass::Text => Kind::Text,
            ClipClass::Link { .. } => Kind::Link,
            ClipClass::Markdown => Kind::Markdown,
            ClipClass::Code { .. } => Kind::Code,
            ClipClass::Css => Kind::Css,
            ClipClass::Color { .. } => Kind::Color,
            ClipClass::Image { .. } => Kind::Image,
            ClipClass::Files { .. } => Kind::Files,
        }
    }

    /// Words a search matches besides the content (kind, language, host,
    /// notation, "screenshot").
    pub fn keywords(&self) -> String {
        let kind = self.kind().id();
        match self {
            ClipClass::Link { host } => format!("{kind} url {host}"),
            ClipClass::Code { lang } => format!("{kind} snippet {lang}"),
            ClipClass::Color { notation, rgba } => format!("{kind} colour {notation} #{:02x}{:02x}{:02x}", rgba[0], rgba[1], rgba[2]),
            ClipClass::Image { screenshot: true, .. } => format!("{kind} screenshot"),
            ClipClass::Css => format!("{kind} style"),
            _ => kind.to_string(),
        }
    }
}

/// The first `max` bytes of `s`, cut at a character boundary.
fn head(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Classifies copied text.
pub fn classify_text(text: &str) -> ClipClass {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return ClipClass::Text;
    }
    let whole = trimmed.len() <= SCAN_BYTES;
    let t = head(trimmed, SCAN_BYTES);
    if whole {
        if let Some(c) = color::parse(t) {
            return ClipClass::Color { notation: c.notation.to_string(), rgba: c.rgba };
        }
        if let Some(uris) = file_uris(t) {
            return ClipClass::Files { count: uris.len() };
        }
        if let Some(host) = link_host(t) {
            return ClipClass::Link { host };
        }
        if let Some(lang) = fenced(t) {
            return ClipClass::Code { lang };
        }
    }
    if css(t) {
        return ClipClass::Css;
    }
    let md = markdown_score(t);
    let code = lang::detect(t);
    if md >= MD_SURE {
        return ClipClass::Markdown;
    }
    if let Some(found) = code
        && found.sure
    {
        return ClipClass::Code { lang: found.lang.to_string() };
    }
    if md >= MD_LIKELY {
        return ClipClass::Markdown;
    }
    match code {
        Some(found) => ClipClass::Code { lang: found.lang.to_string() },
        None => ClipClass::Text,
    }
}

// ── Links and files ──────────────────────────────────────────────────────

/// Schemes whose address has a host.
const HOST_SCHEMES: &[&str] = &["https://", "http://", "ftp://", "ftps://", "sftp://", "ssh://", "ws://", "wss://", "git://"];

/// The host of a single link (`https://…`, `www.…`, `mailto:…`,
/// `git@host:…`), lower-cased; `None` when the text is anything more.
pub fn link_host(t: &str) -> Option<String> {
    let t = t.trim();
    if t.is_empty() || t.contains(char::is_whitespace) {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    if let Some(scheme) = HOST_SCHEMES.iter().find(|s| lower.starts_with(*s)) {
        return host(&t[scheme.len()..]);
    }
    if let Some(rest) = lower.strip_prefix("mailto:") {
        let addr = rest.split('?').next()?;
        let (user, domain) = addr.rsplit_once('@')?;
        return (!user.is_empty()).then(|| valid_host(domain)).flatten();
    }
    if lower.starts_with("www.") {
        return host(t);
    }
    if let Some(rest) = t.strip_prefix("git@") {
        let (h, path) = rest.split_once(':')?;
        return (!path.is_empty()).then(|| valid_host(h)).flatten();
    }
    None
}

fn host(rest: &str) -> Option<String> {
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let h = if authority.starts_with('[') { authority.split_inclusive(']').next()? } else { authority.split(':').next()? };
    valid_host(h)
}

fn valid_host(h: &str) -> Option<String> {
    let ok = !h.is_empty()
        && !h.starts_with(['.', '-'])
        && !h.ends_with(['.', '-'])
        && !h.contains("..")
        && (h.contains('.') || h.eq_ignore_ascii_case("localhost") || h.starts_with('['))
        && h.chars().all(|c| c.is_alphanumeric() || "-.[]:".contains(c));
    ok.then(|| h.to_lowercase())
}

/// A `text/uri-list` (or plain text) made only of `file://` URIs.
pub fn file_uris(text: &str) -> Option<Vec<String>> {
    let uris: Vec<String> = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(String::from).collect();
    (!uris.is_empty() && uris.iter().all(|u| u.starts_with("file://") && u.len() > "file://".len())).then_some(uris)
}

/// The local path of a `file://` URI, percent-decoded.
pub fn uri_path(uri: &str) -> PathBuf {
    let rest = uri.strip_prefix("file://").unwrap_or(uri);
    // `file://host/path`: drop the host part.
    let path = if rest.starts_with('/') { rest } else { rest.find('/').map_or(rest, |i| &rest[i..]) };
    PathBuf::from(percent_decode(path))
}

/// Percent-encodes a path for a `file://` URI (RFC 8089 / 3986: unreserved
/// characters and `/` stay).
pub fn path_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ── Code fences and Markdown ─────────────────────────────────────────────

/// Markdown at or above this score wins even over decisive code.
const MD_SURE: u32 = 3;
/// Markdown at or above this score wins over code that isn't decisive.
const MD_LIKELY: u32 = 2;
/// List items needed for a list to count.
const MD_MIN_LIST_ITEMS: usize = 2;

fn fence(l: &str) -> bool {
    let l = l.trim_start();
    l.starts_with("```") || l.starts_with("~~~")
}

/// A copy that is exactly one fenced code block: code, in the fence's
/// language (or the detected one).
fn fenced(t: &str) -> Option<String> {
    let lines: Vec<&str> = t.lines().collect();
    if lines.len() < 2 || !fence(lines[0]) || !fence(lines[lines.len() - 1]) || lines.iter().filter(|l| fence(l)).count() != 2 {
        return None;
    }
    let info = lines[0].trim().trim_start_matches(['`', '~']).trim();
    let lang = info.split(|c: char| c.is_whitespace() || c == ',' || c == '{').next().unwrap_or("").to_ascii_lowercase();
    if !lang.is_empty() {
        return Some(lang);
    }
    let inner = lines[1..lines.len() - 1].join("\n");
    Some(lang::detect(&inner).map(|f| f.lang.to_string()).unwrap_or_default())
}

fn is_heading(l: &str) -> bool {
    let hashes = l.bytes().take_while(|b| *b == b'#').count();
    (1..=6).contains(&hashes) && l[hashes..].starts_with(' ') && !l[hashes..].trim().is_empty()
}

fn is_list_item(l: &str) -> bool {
    let l = l.trim_start();
    if let Some(rest) = l.strip_prefix(['-', '*', '+']) {
        return rest.starts_with(' ') && !rest.trim().is_empty();
    }
    let digits = l.bytes().take_while(u8::is_ascii_digit).count();
    (1..=3).contains(&digits) && l[digits..].starts_with(['.', ')']) && l[digits + 1..].starts_with(' ') && !l[digits + 1..].trim().is_empty()
}

/// `[label](target)` on a line.
fn has_link(l: &str) -> bool {
    l.find("](").is_some_and(|i| l[..i].contains('[') && l[i + 2..].contains(')'))
}

/// A closed `**strong**` run on a line.
fn has_strong(l: &str) -> bool {
    let mut parts = l.split("**");
    parts.next();
    let inner: Vec<&str> = parts.collect();
    // `a **b** c` splits into ["a ", "b", " c"]: an odd number of pieces after the first.
    inner.len() >= 2 && inner[0].chars().next().is_some_and(|c| !c.is_whitespace()) && inner[0].chars().last().is_some_and(|c| !c.is_whitespace())
}

/// A closed `` `code` `` span on a line.
fn has_inline_code(l: &str) -> bool {
    let n = l.matches('`').count();
    n >= 2 && n.is_multiple_of(2) && !l.contains("``")
}

fn is_table_rule(l: &str) -> bool {
    let l = l.trim();
    l.contains('-') && l.contains('|') && l.chars().all(|c| "|-: ".contains(c)) && l.matches('-').count() >= 3
}

/// How Markdown-like `t` is: one point per distinct feature (headings
/// followed by a blank line, task lists, tables and prose around a fenced
/// block count more). Code comment lines are skipped.
fn markdown_score(t: &str) -> u32 {
    let lines: Vec<&str> = t.lines().collect();
    let (mut heading, mut lists, mut task, mut link, mut strong, mut code, mut quote, mut table, mut fences, mut prose) = (0u32, 0usize, false, false, false, false, false, false, 0u32, false);
    for (i, raw) in lines.iter().enumerate() {
        let l = raw.trim_start();
        if l.starts_with("//") || l.starts_with("/*") {
            continue;
        }
        if fence(l) {
            fences += 1;
            continue;
        }
        if !fences.is_multiple_of(2) {
            continue;
        }
        if is_heading(l) {
            let spaced = lines.get(i + 1).is_none_or(|n| n.trim().is_empty());
            heading = heading.max(if spaced && lines.len() > 1 { 2 } else { 1 });
            continue;
        }
        if is_list_item(l) {
            lists += 1;
            let rest = l.trim_start_matches(['-', '*', '+']).trim_start();
            task |= rest.starts_with("[ ] ") || rest.starts_with("[x] ") || rest.starts_with("[X] ");
        }
        link |= has_link(l);
        strong |= has_strong(l);
        code |= has_inline_code(l);
        quote |= l.starts_with("> ");
        table |= is_table_rule(l);
        prose |= !l.is_empty() && !is_list_item(l);
    }
    let mut score = heading;
    score += u32::from(lists >= MD_MIN_LIST_ITEMS) + 2 * u32::from(task) + u32::from(link) + u32::from(strong) + u32::from(code) + u32::from(quote) + 2 * u32::from(table);
    if fences >= 2 && prose {
        score += MD_SURE;
    }
    score
}

// ── CSS ──────────────────────────────────────────────────────────────────

/// Common CSS properties (custom `--props` and vendor prefixes are
/// accepted on top).
const CSS_PROPS: &[&str] = &[
    "align-content", "align-items", "align-self", "all", "animation", "animation-delay", "animation-duration", "animation-name", "aspect-ratio", "backdrop-filter", "background", "background-color",
    "background-image", "background-position", "background-repeat", "background-size", "border", "border-bottom", "border-collapse", "border-color", "border-left", "border-radius", "border-right",
    "border-style", "border-top", "border-width", "bottom", "box-shadow", "box-sizing", "caret-color", "clip-path", "color", "column-gap", "columns", "content", "cursor", "display", "fill", "filter",
    "flex", "flex-basis", "flex-direction", "flex-flow", "flex-grow", "flex-shrink", "flex-wrap", "float", "font", "font-family", "font-feature-settings", "font-size", "font-style",
    "font-variant-numeric", "font-weight", "gap", "grid", "grid-area", "grid-column", "grid-row", "grid-template", "grid-template-areas", "grid-template-columns", "grid-template-rows", "height",
    "inset", "isolation", "justify-content", "justify-items", "justify-self", "left", "letter-spacing", "line-height", "list-style", "margin", "margin-block", "margin-bottom", "margin-inline",
    "margin-left", "margin-right", "margin-top", "mask", "max-height", "max-width", "min-height", "min-width", "mix-blend-mode", "object-fit", "object-position", "opacity", "order", "outline",
    "outline-offset", "overflow", "overflow-x", "overflow-y", "padding", "padding-block", "padding-bottom", "padding-inline", "padding-left", "padding-right", "padding-top", "place-items",
    "pointer-events", "position", "resize", "right", "rotate", "row-gap", "scale", "scroll-behavior", "stroke", "stroke-width", "text-align", "text-decoration", "text-overflow", "text-shadow",
    "text-transform", "top", "transform", "transform-origin", "transition", "transition-duration", "translate", "user-select", "vertical-align", "visibility", "white-space", "width", "will-change",
    "word-break", "word-wrap", "z-index",
];
/// Vendor prefixes stripped before the property lookup.
const VENDOR_PREFIXES: &[&str] = &["-webkit-", "-moz-", "-ms-", "-o-"];
/// A declaration value with more plain words than this is a sentence.
const CSS_MAX_PLAIN_WORDS: usize = 4;

fn css_property(p: &str) -> bool {
    if let Some(custom) = p.strip_prefix("--") {
        return !custom.is_empty() && custom.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    }
    let p = VENDOR_PREFIXES.iter().find_map(|v| p.strip_prefix(v)).unwrap_or(p);
    CSS_PROPS.contains(&p)
}

/// `property: value` (no trailing `;`).
fn css_decl(d: &str) -> bool {
    let Some((p, v)) = d.split_once(':') else { return false };
    let (p, v) = (p.trim(), v.trim());
    if v.is_empty() || !css_property(p) {
        return false;
    }
    let sentence = v.ends_with(['.', '?']) || v.split_whitespace().filter(|w| w.chars().all(char::is_alphabetic)).count() > CSS_MAX_PLAIN_WORDS;
    !sentence
}

fn strip_css_comments(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut rest = t;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        rest = rest[i + 2..].find("*/").map_or("", |j| &rest[i + 2 + j + 2..]);
    }
    out.push_str(rest);
    out
}

/// A rule prelude: a selector list or an at-rule.
fn selector_like(p: &str) -> bool {
    let p = p.trim();
    if p.is_empty() {
        return false;
    }
    if let Some(at) = p.strip_prefix('@') {
        return at.starts_with(|c: char| c.is_ascii_alphabetic());
    }
    if !p.chars().all(|c| c.is_alphanumeric() || "-_.#:*>+~[]=\"' ,()%\n\t|^$".contains(c)) || p.contains("=>") || p.contains("==") {
        return false;
    }
    // `=` only inside attribute selectors, `(` only after a pseudo-class.
    let mut depth = 0i32;
    for (i, c) in p.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => depth -= 1,
            '=' if depth <= 0 => return false,
            '(' if !p[..i].contains(':') => return false,
            _ => {}
        }
    }
    true
}

/// CSS rules (`a:hover { color: red }`, `@media … { … }`) or bare
/// declarations (`margin: 0 auto;`).
fn css(t: &str) -> bool {
    let t = strip_css_comments(t);
    let t = t.trim();
    if t.is_empty() {
        return false;
    }
    let decls_ok = |body: &str, any: &mut bool| {
        body.split(';').map(str::trim).filter(|d| !d.is_empty()).all(|d| {
            *any = true;
            css_decl(d)
        })
    };
    let mut any = false;
    if !t.contains('{') {
        return !t.contains('}') && t.split(['\n', ';']).map(str::trim).filter(|d| !d.is_empty()).all(css_decl) && t.split(['\n', ';']).any(|d| !d.trim().is_empty());
    }
    let (mut depth, mut buf) = (0usize, String::new());
    for c in t.chars() {
        match c {
            '{' => {
                // Declarations before a nested rule, then its prelude.
                let (decls, prelude) = buf.rsplit_once(';').unwrap_or(("", buf.as_str()));
                if !selector_like(prelude) || (depth > 0 && !decls_ok(decls, &mut any)) || (depth == 0 && !decls.trim().is_empty()) {
                    return false;
                }
                buf.clear();
                depth += 1;
            }
            '}' => {
                if depth == 0 || !decls_ok(&buf, &mut any) {
                    return false;
                }
                buf.clear();
                depth -= 1;
            }
            ';' if depth == 0 => {
                // Top-level statements are at-rules (`@import url(x);`).
                if !buf.trim().starts_with('@') {
                    return false;
                }
                any = true;
                buf.clear();
            }
            _ => buf.push(c),
        }
    }
    depth == 0 && buf.trim().is_empty() && any
}

// ── Images ───────────────────────────────────────────────────────────────

/// Screenshot tools, matched against the focused app and a PNG's
/// `Software` text.
const SCREENSHOT_TOOLS: &[&str] = &["grim", "grimblast", "hyprshot", "flameshot", "spectacle", "gnome-screenshot", "screenshot", "ksnip", "swappy", "satty", "shotman", "gradia", "wayshot", "watershot"];
/// Common display modes (physical pixels): an image exactly this size is a
/// full-screen capture.
const DISPLAY_SIZES: &[(u32, u32)] = &[
    (1280, 720), (1280, 800), (1366, 768), (1440, 900), (1536, 864), (1600, 900), (1680, 1050), (1920, 1080), (1920, 1200), (2048, 1152), (2160, 1440), (2256, 1504), (2560, 1080), (2560, 1440),
    (2560, 1600), (2880, 1620), (2880, 1800), (3000, 2000), (3024, 1964), (3440, 1440), (3840, 1600), (3840, 2160), (3840, 2400), (5120, 1440), (5120, 2160), (5120, 2880), (6016, 3384), (7680, 4320),
];
/// PNG signature and the chunk that ends the metadata we read.
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Classifies a copied image from its bytes (only the header is decoded).
pub fn classify_image(bytes: &[u8], app_id: &str) -> ClipClass {
    let (width, height) = png_size(bytes)
        .or_else(|| image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok().and_then(|r| r.into_dimensions().ok()))
        .unwrap_or((0, 0));
    let app = app_id.to_ascii_lowercase();
    let by_tool = |s: &str| SCREENSHOT_TOOLS.iter().any(|t| s.contains(t));
    let screenshot = (!app.is_empty() && by_tool(&app)) || png_software(bytes).is_some_and(|s| by_tool(&s)) || DISPLAY_SIZES.contains(&(width, height));
    ClipClass::Image { width, height, screenshot }
}

/// Classifies a saved image file (reads only its first bytes).
pub fn classify_image_file(path: &Path, app_id: &str) -> ClipClass {
    let mut head = Vec::new();
    if let Ok(f) = std::fs::File::open(path) {
        let _ = f.take(IMAGE_HEAD_BYTES).read_to_end(&mut head);
    }
    classify_image(&head, app_id)
}

/// A PNG's size from its IHDR chunk (always first, right after the
/// signature: 4-byte length, `IHDR`, then width and height).
fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let at = PNG_SIGNATURE.len();
    if !bytes.starts_with(PNG_SIGNATURE) || bytes.get(at + 4..at + 8)? != b"IHDR" {
        return None;
    }
    let word = |i: usize| bytes.get(i..i + 4).and_then(|w| w.try_into().ok()).map(u32::from_be_bytes);
    Some((word(at + 8)?, word(at + 12)?))
}

/// A PNG's `Software` text (tEXt / iTXt before the image data), lower-cased.
fn png_software(bytes: &[u8]) -> Option<String> {
    let mut at = bytes.strip_prefix(PNG_SIGNATURE).map(|_| PNG_SIGNATURE.len())?;
    // Chunk: 4-byte length, 4-byte type, data, 4-byte CRC.
    while let (Some(len), Some(kind)) = (bytes.get(at..at + 4), bytes.get(at + 4..at + 8)) {
        let len = u32::from_be_bytes(len.try_into().ok()?) as usize;
        let data = bytes.get(at + 8..at + 8 + len)?;
        match kind {
            b"IDAT" | b"IEND" => return None,
            b"tEXt" | b"iTXt" => {
                let nul = data.iter().position(|b| *b == 0)?;
                if &data[..nul] == b"Software" {
                    let text = if kind == b"iTXt" {
                        // flag, method, language\0, translated keyword\0, text
                        let rest = data.get(nul + 3..)?;
                        let lang_end = rest.iter().position(|b| *b == 0)?;
                        let rest = &rest[lang_end + 1..];
                        let tr_end = rest.iter().position(|b| *b == 0)?;
                        &rest[tr_end + 1..]
                    } else {
                        &data[nul + 1..]
                    };
                    return Some(String::from_utf8_lossy(text).to_lowercase());
                }
            }
            _ => {}
        }
        at += 8 + len + 4;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(t: &str) -> Kind {
        classify_text(t).kind()
    }

    #[test]
    fn plain_text() {
        for t in [
            "hello world",
            "Let me know when you are free tomorrow.",
            "Meeting at 5 => bring snacks",
            "TODO: buy milk",
            "Name: Ann\nPhone: 555 0100",
            "#123",
            "#2048 fixed the crash",
            "red",
            "example.com",
            "notes.txt",
            "/home/demo/Documents/report.pdf",
            "**not closed",
            "- just one item",
            "I said `hi` once.",
            "1. First, we went home",
            "time: 10:30",
            "Display: none of these",
            "color: my favourite is blue.",
            "http://",
            "https:// spaced.com",
            "mailto:@nobody",
            "+20 100 123 4567",
            "",
            "   ",
        ] {
            assert_eq!(kind(t), Kind::Text, "{t:?} → {:?}", classify_text(t));
        }
    }

    #[test]
    fn links_with_hosts() {
        let cases = [
            ("https://github.com/theHamdiz/mandara", "github.com"),
            ("http://localhost:8080/health", "localhost"),
            ("https://user:pw@Example.ORG:443/a?b#c", "example.org"),
            ("www.mandara.dev/docs", "www.mandara.dev"),
            ("mailto:hello@mandara.dev?subject=hi", "mandara.dev"),
            ("git@github.com:theHamdiz/mandara.git", "github.com"),
            ("ssh://git@codeberg.org/x/y", "codeberg.org"),
            ("https://[::1]:3000/", "[::1]"),
            ("  https://lrclib.net/api  ", "lrclib.net"),
        ];
        for (t, host) in cases {
            assert_eq!(classify_text(t), ClipClass::Link { host: host.into() }, "{t:?}");
        }
        for t in ["https://github.com and more", "https://.bad", "https://bad..host/x", "ftp://", "see https://x.org"] {
            assert_ne!(kind(t), Kind::Link, "{t:?}");
        }
    }

    #[test]
    fn colours() {
        for t in ["#cba6f7", "#FFF", "#ff880080", "rgb(255, 136, 0)", "rgba(0,0,0,.5)", "hsl(220 80% 60%)", "hwb(200 10% 20%)", "lab(50% 40 -20)", "lch(60 50 120)", "oklab(0.7 0.1 -0.1)", "oklch(70% 0.15 250)", "rgba(cba6f7ee)", "#1e1e2e;"] {
            assert_eq!(kind(t), Kind::Color, "{t:?}");
        }
        let ClipClass::Color { notation, rgba } = classify_text("oklch(70% 0.15 250)") else { panic!() };
        assert_eq!(notation, "oklch");
        assert!(rgba[2] > rgba[0], "a blue: {rgba:?}");
        // A colour inside a sentence or a declaration is not a colour value.
        assert_ne!(kind("#ff8800 is orange"), Kind::Color);
        assert_eq!(kind("color: #ff8800;"), Kind::Css);
    }

    #[test]
    fn markdown() {
        for t in [
            "# Plan\n\nShip the clipboard.",
            "- [ ] milk\n- [x] bread",
            "Read the [docs](https://mandara.dev) and run `cargo test`.",
            "## Setup\n- install\n- configure",
            "| a | b |\n|---|---|\n| 1 | 2 |",
            "Some **bold** words and a [link](x).",
            "> quoted\n\n- one\n- two",
            "Intro paragraph.\n\n```rust\nfn main() {}\n```\n\nMore text.",
            "1. first\n2. second\n\n**Note:** done",
        ] {
            assert_eq!(kind(t), Kind::Markdown, "{t:?} → {:?}", classify_text(t));
        }
    }

    #[test]
    fn code_with_languages() {
        let cases = [
            ("fn main() {\n    println!(\"hi\");\n}", "rust"),
            ("```python\nprint('hi')\n```", "python"),
            ("```\nconsole.log(1)\n```", "javascript"),
            ("def f(x):\n    return x * 2", "python"),
            ("{\"a\": [1, 2]}", "json"),
            ("SELECT id FROM users;", "sql"),
            ("sudo systemctl restart greetd", "shell"),
            ("# Install deps\nimport os\n\ndef main():\n    print(os.getcwd())", "python"),
            ("/// Adds [`x`](Foo) and `y`.\npub fn add(x: i32, y: i32) -> i32 { x + y }", "rust"),
        ];
        for (t, lang) in cases {
            assert_eq!(classify_text(t), ClipClass::Code { lang: lang.into() }, "{t:?}");
        }
    }

    #[test]
    fn css_rules_and_declarations() {
        for t in [
            "display: flex",
            "margin: 0 auto;",
            "--accent: #cba6f7;",
            "color: red;\nbackground: #000;",
            "-webkit-backdrop-filter: blur(10px);",
            ".card { border-radius: 12px; padding: 8px 12px; }",
            "a:hover,\na:focus-visible {\n  color: var(--accent);\n}",
            "@media (max-width: 600px) {\n  .bar { display: none; }\n}",
            "@import url(\"x.css\");\nbody { margin: 0 }",
            "/* theme */\n:root { --bg: oklch(20% 0.02 260); }",
            "input[type=\"text\"] { width: 100%; }",
        ] {
            assert_eq!(kind(t), Kind::Css, "{t:?} → {:?}", classify_text(t));
        }
        // Near misses: code with braces, YAML, prose with a colon.
        for t in ["const style = { color: 'red' };", "pub struct Point { x: f32, y: f32 }", "if (x) { y: z }", "Note: buy milk", "Display: flex", "server:\n  color: red", "{ color: red }"] {
            assert_ne!(kind(t), Kind::Css, "{t:?}");
        }
    }

    #[test]
    fn files_from_uri_lists() {
        let list = "file:///home/demo/Pictures/harbour%20at%20dusk.png\r\nfile:///home/demo/Documents/notes.md\r\n";
        assert_eq!(classify_text(list), ClipClass::Files { count: 2 });
        assert_eq!(uri_path("file:///home/demo/Pictures/harbour%20at%20dusk.png"), PathBuf::from("/home/demo/Pictures/harbour at dusk.png"));
        assert_eq!(uri_path("file://localhost/etc/hosts"), PathBuf::from("/etc/hosts"));
        assert_eq!(uri_path(&path_uri(Path::new("/tmp/a b/ü.txt"))), PathBuf::from("/tmp/a b/ü.txt"));
        assert_eq!(file_uris("# comment\nfile:///a\n"), Some(vec!["file:///a".to_string()]));
        assert_eq!(file_uris("file:///a\nhttps://x.org"), None);
        assert_eq!(file_uris("file://"), None);
    }

    #[test]
    fn kinds_parse_and_round_trip() {
        for k in Kind::ALL {
            assert_eq!(Kind::parse(k.id()), Some(k));
        }
        assert_eq!(Kind::parse("Colours"), Some(Kind::Color));
        assert_eq!(Kind::parse("screenshots"), Some(Kind::Image));
        assert_eq!(Kind::parse("pinned"), None);
        let c = ClipClass::Code { lang: "rust".into() };
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(json, r#"{"kind":"code","lang":"rust"}"#);
        assert_eq!(serde_json::from_str::<ClipClass>(&json).unwrap(), c);
    }

    /// A PNG with a `Software` text chunk and an IHDR of `w` × `h`.
    fn png(w: u32, h: u32, software: Option<&str>) -> Vec<u8> {
        let chunk = |kind: &[u8], data: &[u8]| {
            let mut c = (data.len() as u32).to_be_bytes().to_vec();
            c.extend_from_slice(kind);
            c.extend_from_slice(data);
            let mut crc_input = kind.to_vec();
            crc_input.extend_from_slice(data);
            c.extend_from_slice(&crc32(&crc_input).to_be_bytes());
            c
        };
        let mut ihdr = w.to_be_bytes().to_vec();
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        let mut out = PNG_SIGNATURE.to_vec();
        out.extend(chunk(b"IHDR", &ihdr));
        if let Some(s) = software {
            out.extend(chunk(b"tEXt", format!("Software\0{s}").as_bytes()));
        }
        out.extend(chunk(b"IEND", &[]));
        out
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for b in bytes {
            crc ^= u32::from(*b);
            for _ in 0..8 {
                crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }

    #[test]
    fn images_and_screenshots() {
        assert_eq!(classify_image(&png(3840, 2160, None), ""), ClipClass::Image { width: 3840, height: 2160, screenshot: true });
        assert_eq!(classify_image(&png(640, 480, None), ""), ClipClass::Image { width: 640, height: 480, screenshot: false });
        assert_eq!(classify_image(&png(812, 377, Some("Spectacle")), ""), ClipClass::Image { width: 812, height: 377, screenshot: true });
        assert_eq!(classify_image(&png(812, 377, Some("GIMP 2.10")), ""), ClipClass::Image { width: 812, height: 377, screenshot: false });
        assert!(matches!(classify_image(&png(500, 300, None), "org.flameshot.Flameshot"), ClipClass::Image { screenshot: true, .. }));
        // Unreadable bytes still make an image entry, size unknown.
        assert_eq!(classify_image(b"not an image", ""), ClipClass::Image { width: 0, height: 0, screenshot: false });
    }
}
