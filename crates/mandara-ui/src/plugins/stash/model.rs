//! Stash's data, as stored and as planned: shelves and their items, how a
//! dropped MIME type becomes an item, what a drag out offers, the cache's
//! eviction rules and the command-line targets. Pure: no GPUI, no I/O, so
//! all of it is unit-tested.
//!
//! **Persistence.** `$XDG_STATE_HOME/mandara/stash.json` holds the open
//! shelves (position, size, output, view, minimized) and their items, plus
//! a short list of closed shelves that can be restored. Files and folders
//! are kept by reference (their path); pictures, text, HTML and other data
//! dropped as bytes live in the stash's own folder,
//! `$XDG_CACHE_HOME/mandara/stash/<shelf>/`. A closed shelf keeps its
//! folder until the cache limit evicts it (oldest first); an open shelf's
//! files are never evicted without asking.

use std::path::{Path, PathBuf};

use mandara_config::stash::ShelfView;
use serde::{Deserialize, Serialize};

/// Format of `stash.json`.
pub const STATE_VERSION: u32 = 1;
/// Closed shelves kept for `stash restore`.
pub const CLOSED_KEEP: usize = 12;
/// Characters of a text item kept in the state file as its preview.
pub const PREVIEW_CHARS: usize = 280;
/// Largest drop read into the stash (a picture, text or data), bytes.
pub const MAX_DROP_BYTES: u64 = 256 * 1024 * 1024;
/// Most items one drop may add (a folder of thousands stays one drop).
pub const MAX_ITEMS_PER_DROP: usize = 500;
/// Largest picture fetched from a link, bytes.
pub const MAX_FETCH_BYTES: u64 = 32 * 1024 * 1024;

/// Everything Stash keeps across restarts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StashState {
    pub version: u32,
    /// The next shelf / item id (ids are never reused).
    pub next_id: u64,
    pub shelves: Vec<Shelf>,
    /// Most recently closed last.
    pub closed: Vec<ClosedShelf>,
}

/// A floating shelf.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shelf {
    pub id: u64,
    /// Seconds since the epoch.
    pub created: u64,
    /// The output it floats on (`None`: the focused one).
    #[serde(default)]
    pub output: Option<String>,
    /// Top-left corner on that output, logical px.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    #[serde(default)]
    pub view: ShelfView,
    #[serde(default)]
    pub minimized: bool,
    /// Items dragged out leave the shelf (else they stay).
    #[serde(default)]
    pub move_out: bool,
    #[serde(default)]
    pub items: Vec<Item>,
}

/// A shelf closed with its items, restorable until its files are evicted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClosedShelf {
    pub shelf: Shelf,
    /// Seconds since the epoch.
    pub closed: u64,
}

/// One thing on a shelf.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: u64,
    /// Seconds since the epoch.
    pub added: u64,
    #[serde(flatten)]
    pub kind: ItemKind,
}

/// What an item holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ItemKind {
    /// A file or folder, by reference.
    File {
        path: PathBuf,
        #[serde(default)]
        dir: bool,
        /// Its MIME type, found when it was dropped.
        #[serde(default)]
        mime: String,
    },
    /// A picture kept in the stash's folder.
    Image {
        /// File name in the shelf's folder.
        blob: String,
        mime: String,
        /// Where it came from (a link or the original file), if known.
        #[serde(default)]
        source: Option<String>,
    },
    /// Text (and the HTML it came with), kept in the stash's folder.
    Text {
        blob: String,
        /// The first [`PREVIEW_CHARS`] characters, for the tile.
        preview: String,
        /// Characters in all.
        chars: u64,
        /// The HTML version, if the drop had one.
        #[serde(default)]
        html: Option<String>,
    },
    /// A web link.
    Url {
        url: String,
        #[serde(default)]
        title: Option<String>,
        /// The site's icon, once fetched (file name in the shelf's folder).
        #[serde(default)]
        icon: Option<String>,
    },
    /// Anything else, as raw bytes with its MIME type.
    Data { blob: String, mime: String, bytes: u64 },
}

impl ItemKind {
    /// The kind's id (`file`, `image`, …) for lists and commands.
    pub fn id(&self) -> &'static str {
        match self {
            ItemKind::File { dir: true, .. } => "folder",
            ItemKind::File { .. } => "file",
            ItemKind::Image { .. } => "image",
            ItemKind::Text { .. } => "text",
            ItemKind::Url { .. } => "url",
            ItemKind::Data { .. } => "data",
        }
    }

    /// The stash files it owns (in its shelf's folder).
    pub fn blobs(&self) -> impl Iterator<Item = &str> {
        let (a, b) = match self {
            ItemKind::File { .. } => (None, None),
            ItemKind::Image { blob, .. } | ItemKind::Data { blob, .. } => (Some(blob.as_str()), None),
            ItemKind::Text { blob, html, .. } => (Some(blob.as_str()), html.as_deref()),
            ItemKind::Url { icon, .. } => (icon.as_deref(), None),
        };
        a.into_iter().chain(b)
    }
}

impl Item {
    /// A short name for tiles and lists.
    pub fn title(&self) -> String {
        match &self.kind {
            ItemKind::File { path, .. } => path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned()),
            ItemKind::Image { source, .. } => source.as_deref().and_then(last_segment).map(str::to_string).unwrap_or_else(|| mandara_i18n::t("Picture").to_string()),
            ItemKind::Text { preview, .. } => first_line(preview, 60),
            ItemKind::Url { url, title, .. } => title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| host_of(url).unwrap_or(url).to_string()),
            ItemKind::Data { mime, .. } => mime.clone(),
        }
    }
}

impl Shelf {
    pub fn new(id: u64, created: u64, output: Option<String>, (x, y): (f32, f32), (w, h): (f32, f32), view: ShelfView) -> Self {
        Self { id, created, output, x, y, w, h, view, minimized: false, move_out: false, items: Vec::new() }
    }

    pub fn item(&self, id: u64) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }
}

impl StashState {
    /// A fresh id.
    pub fn take_id(&mut self) -> u64 {
        // Ids start at 1 (0 is "none" in commands).
        self.next_id = self.next_id.max(1);
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn shelf(&self, id: u64) -> Option<&Shelf> {
        self.shelves.iter().find(|s| s.id == id)
    }

    pub fn shelf_mut(&mut self, id: u64) -> Option<&mut Shelf> {
        self.shelves.iter_mut().find(|s| s.id == id)
    }

    /// Repairs a loaded state: ids above every id in use, no duplicate
    /// shelves, sizes and positions within limits, closed list bounded.
    pub fn sanitize(&mut self) {
        use mandara_config::stash::*;
        self.version = STATE_VERSION;
        let mut seen = std::collections::BTreeSet::new();
        self.shelves.retain(|s| seen.insert(s.id));
        let max_id = self.shelves.iter().chain(self.closed.iter().map(|c| &c.shelf)).flat_map(|s| std::iter::once(s.id).chain(s.items.iter().map(|i| i.id))).max().unwrap_or(0);
        self.next_id = self.next_id.max(max_id + 1).max(1);
        for s in self.shelves.iter_mut().chain(self.closed.iter_mut().map(|c| &mut c.shelf)) {
            let finite = |v: f32, d: f32| if v.is_finite() { v } else { d };
            s.w = finite(s.w, SHELF_DEFAULT_W).clamp(SHELF_MIN_W, SHELF_MAX_W);
            s.h = finite(s.h, SHELF_DEFAULT_H).clamp(SHELF_MIN_H, SHELF_MAX_H);
            s.x = finite(s.x, 0.0).max(0.0);
            s.y = finite(s.y, 0.0).max(0.0);
        }
        if self.closed.len() > CLOSED_KEEP {
            let extra = self.closed.len() - CLOSED_KEEP;
            self.closed.drain(..extra);
        }
    }

    /// Items in every open shelf.
    pub fn item_count(&self) -> usize {
        self.shelves.iter().map(|s| s.items.len()).sum()
    }

    /// The most recently created open shelf that is not minimized, else the
    /// most recent one.
    pub fn latest(&self) -> Option<u64> {
        self.shelves.iter().rev().find(|s| !s.minimized).or_else(|| self.shelves.last()).map(|s| s.id)
    }

    /// Closes shelf `id`: it moves to the closed list (restorable), or with
    /// `empty` it is forgotten. Returns the closed shelf and the shelves
    /// that dropped off the end of the closed list (their folders can go).
    pub fn close(&mut self, id: u64, empty: bool, now: u64) -> Option<(Shelf, Vec<u64>)> {
        let at = self.shelves.iter().position(|s| s.id == id)?;
        let shelf = self.shelves.remove(at);
        let mut gone = Vec::new();
        if empty || shelf.items.is_empty() {
            gone.push(shelf.id);
        } else {
            self.closed.push(ClosedShelf { shelf: shelf.clone(), closed: now });
            while self.closed.len() > CLOSED_KEEP {
                gone.push(self.closed.remove(0).shelf.id);
            }
        }
        Some((shelf, gone))
    }

    /// Reopens closed shelf `id` (or the last one closed).
    pub fn restore(&mut self, id: Option<u64>) -> Option<u64> {
        let at = match id {
            Some(id) => self.closed.iter().position(|c| c.shelf.id == id)?,
            None => self.closed.len().checked_sub(1)?,
        };
        let mut shelf = self.closed.remove(at).shelf;
        shelf.minimized = false;
        let id = shelf.id;
        self.shelves.push(shelf);
        Some(id)
    }
}

// ── What a drop becomes ──────────────────────────────────────────────────

pub const URI_LIST: &str = "text/uri-list";
pub const MOZ_URL: &str = "text/x-moz-url";
pub const TEXT_UTF8: &str = "text/plain;charset=utf-8";
pub const TEXT_PLAIN: &str = "text/plain";
pub const UTF8_STRING: &str = "UTF8_STRING";
pub const HTML: &str = "text/html";

/// Picture types read straight from a drop, best first.
const IMAGE_TYPES: [&str; 6] = ["image/png", "image/webp", "image/jpeg", "image/gif", "image/bmp", "image/svg+xml"];
/// Types that are only a program's own bookkeeping, never worth an item.
fn private_type(mime: &str) -> bool {
    let m = mime.to_ascii_lowercase();
    m.starts_with("application/x-moz-")
        || m.starts_with("chromium/")
        || m.starts_with("application/x-qt-")
        || m.starts_with("application/x-kde-")
        || m.starts_with("x-special/")
        || m == "text/_moz_htmlcontext"
        || m == "text/_moz_htmlinfo"
        || m == "application/x-gtk-text-buffer-rich-text"
        || m == "save_targets"
        || m == "targets"
        || m == "multiple"
        || m == "timestamp"
        || m == "text"
        || m == "string"
        || m == "compound_text"
        || m == "text/x-moz-url-priv"
        || m == "text/x-moz-url-data"
        || m == "text/x-moz-url-desc"
        || m.starts_with("text/x-moz-message")
        || m == "application/x-mandara-stash"
}

/// How to read a data drop (not local files).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropPlan {
    /// A picture's bytes under this type.
    Image(String),
    /// Links: read this type (`text/x-moz-url` gives titles, `text/uri-list`
    /// one link a line).
    Links(String),
    /// Text, with the HTML type to keep alongside when offered.
    Text { text: String, html: Option<String> },
    /// HTML only (kept as text, and its markup as HTML).
    Html(String),
    /// Raw data of this type.
    Data(String),
}

/// Picks what to read from a drop offering `mime_types` (in the sender's
/// order): pictures first, then links, then text (with its HTML), then
/// HTML alone, then the first meaningful other type.
pub fn plan_drop(mime_types: &[impl AsRef<str>]) -> Option<DropPlan> {
    let has = |m: &str| mime_types.iter().find(|t| t.as_ref().eq_ignore_ascii_case(m)).map(|t| t.as_ref().to_string());
    if let Some(m) = IMAGE_TYPES.iter().find_map(|m| has(m)) {
        return Some(DropPlan::Image(m));
    }
    if let Some(m) = has(MOZ_URL).or_else(|| has(URI_LIST)) {
        return Some(DropPlan::Links(m));
    }
    let html = has(HTML);
    if let Some(text) = has(TEXT_UTF8).or_else(|| has(UTF8_STRING)).or_else(|| has(TEXT_PLAIN)).or_else(|| mime_types.iter().find(|t| t.as_ref().to_ascii_lowercase().starts_with("text/plain")).map(|t| t.as_ref().to_string())) {
        return Some(DropPlan::Text { text, html });
    }
    if let Some(html) = html {
        return Some(DropPlan::Html(html));
    }
    if let Some(m) = mime_types.iter().map(AsRef::as_ref).find(|m| m.starts_with("image/") && !private_type(m)) {
        return Some(DropPlan::Image(m.to_string()));
    }
    mime_types.iter().map(AsRef::as_ref).find(|m| !private_type(m) && m.contains('/')).map(|m| DropPlan::Data(m.to_string()))
}

/// A link read from a drop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub url: String,
    pub title: Option<String>,
}

/// Parses `text/x-moz-url` (URL and title on alternating lines) or
/// `text/uri-list` (one URI a line, `#` comments). Firefox sends the
/// former as UTF-16; both encodings are read.
pub fn parse_links(mime: &str, bytes: &[u8]) -> Vec<Link> {
    let text = decode_text(bytes);
    let lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    if mime.eq_ignore_ascii_case(MOZ_URL) {
        let lines: Vec<&str> = lines.collect();
        return lines.chunks(2).filter(|c| !c[0].starts_with('#')).map(|c| Link { url: c[0].to_string(), title: c.get(1).map(|t| t.to_string()).filter(|t| !t.is_empty() && *t != c[0]) }).take(MAX_ITEMS_PER_DROP).collect();
    }
    lines.filter(|l| !l.starts_with('#')).map(|l| Link { url: l.to_string(), title: None }).take(MAX_ITEMS_PER_DROP).collect()
}

/// Text from a drop: UTF-16 (with or without a byte-order mark, as some
/// programs send it) or UTF-8, with Windows line ends unified and the NUL
/// terminator some senders add removed.
pub fn decode_text(bytes: &[u8]) -> String {
    let utf16 = |le: bool, b: &[u8]| -> String {
        let units = b.as_chunks::<2>().0.iter().map(|c| if le { u16::from_le_bytes(*c) } else { u16::from_be_bytes(*c) });
        char::decode_utf16(units).map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER)).collect()
    };
    let text = match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(true, rest),
        [0xFE, 0xFF, rest @ ..] => utf16(false, rest),
        // UTF-16LE without a mark: every other byte zero in ASCII text.
        b if b.len() >= 4 && b.len().is_multiple_of(2) && b[1] == 0 && b[3] == 0 && b[0] != 0 => utf16(true, b),
        b => String::from_utf8_lossy(b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b)).into_owned(),
    };
    let text = if text.contains('\r') { text.replace("\r\n", "\n") } else { text };
    text.trim_end_matches('\0').to_string()
}

/// A preview of `text`: its first [`PREVIEW_CHARS`] characters.
pub fn preview_of(text: &str) -> String {
    match text.char_indices().nth(PREVIEW_CHARS) {
        Some((at, _)) => text[..at].to_string(),
        None => text.to_string(),
    }
}

/// Text from HTML, for a tile: tags and comments removed, common entities
/// decoded, runs of white space collapsed. Bounded to `max` characters.
pub fn html_to_text(html: &str, max: usize) -> String {
    let mut out = String::with_capacity(html.len().min(max * 4));
    let mut chars = html.chars().peekable();
    let mut in_tag = false;
    let mut skip_until: Option<&'static str> = None;
    let mut tag = String::new();
    let mut space = false;
    let mut count = 0;
    while let Some(c) = chars.next() {
        if count >= max {
            break;
        }
        if in_tag {
            if c == '>' {
                in_tag = false;
                let name: String = tag.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
                if !tag.starts_with('/') && (name == "script" || name == "style") {
                    skip_until = Some(if name == "script" { "</script" } else { "</style" });
                }
                if matches!(name.as_str(), "br" | "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4") {
                    space = true;
                }
                tag.clear();
            } else {
                tag.push(c);
            }
            continue;
        }
        if c == '<' {
            if let Some(end) = skip_until {
                // Inside <script>/<style>: look for its end tag.
                let rest: String = chars.clone().take(end.len() - 1).collect();
                if format!("<{rest}").eq_ignore_ascii_case(end) {
                    skip_until = None;
                    in_tag = true;
                }
                continue;
            }
            in_tag = true;
            continue;
        }
        if skip_until.is_some() {
            continue;
        }
        let c = if c == '&' {
            let mut entity = String::new();
            while let Some(&n) = chars.peek() {
                if n == ';' || entity.len() > 8 {
                    break;
                }
                entity.push(n);
                chars.next();
            }
            if chars.peek() == Some(&';') {
                chars.next();
            }
            match entity.as_str() {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" | "#39" => '\'',
                "nbsp" => ' ',
                e if e.starts_with('#') => e[1..].strip_prefix(['x', 'X']).map_or_else(|| e[1..].parse::<u32>().ok(), |h| u32::from_str_radix(h, 16).ok()).and_then(char::from_u32).unwrap_or('\u{FFFD}'),
                _ => '\u{FFFD}',
            }
        } else {
            c
        };
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
            count += 1;
        }
        space = false;
        out.push(c);
        count += 1;
    }
    out
}

/// Whether a link points at a picture (by its path's extension).
pub fn looks_like_image(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url).to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".webp", ".gif", ".bmp", ".avif", ".svg"].iter().any(|e| path.ends_with(e))
}

/// Whether a link may be fetched: HTTPS only.
pub fn fetchable(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("https://") && lower.len() > "https://".len() && !lower.contains(char::is_whitespace)
}

/// Whether text is a single web link (then it becomes a link item).
pub fn single_link(text: &str) -> Option<&str> {
    let t = text.trim();
    let lower = t.to_ascii_lowercase();
    ((lower.starts_with("https://") || lower.starts_with("http://")) && !t.contains(char::is_whitespace) && t.len() < 4096).then_some(t)
}

/// The host of an `http(s)` URL.
pub fn host_of(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let host = host.split(':').next()?;
    (!host.is_empty()).then_some(host)
}

fn last_segment(url: &str) -> Option<&str> {
    url.split(['?', '#']).next()?.trim_end_matches('/').rsplit('/').next().filter(|s| !s.is_empty() && !s.contains(':'))
}

fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    match line.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_string(),
    }
}

/// File-name extension for a MIME type (for stash files and drags out).
pub fn extension_for(mime: &str) -> &'static str {
    match mime.split(';').next().unwrap_or(mime).trim().to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        "image/svg+xml" => "svg",
        "text/html" => "html",
        "application/pdf" => "pdf",
        "application/json" => "json",
        m if m.starts_with("text/") => "txt",
        _ => "bin",
    }
}

// ── URIs ─────────────────────────────────────────────────────────────────

/// A `file://` URI's path (percent-decoded), or `None` for other URIs.
pub fn file_uri_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // `file://host/path`: only the local host.
    let path = if rest.starts_with('/') { rest } else { rest.strip_prefix("localhost").filter(|p| p.starts_with('/'))? };
    let bytes = percent_decode(path)?;
    use std::os::unix::ffi::OsStringExt as _;
    Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

fn percent_decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    Some(out)
}

/// `path` as a `file://` URI (RFC 8089, percent-encoded).
pub fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt as _;
    let mut out = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/!$&'()*+,;=:@".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

// ── What a drag out offers ───────────────────────────────────────────────

/// One MIME type of a drag out, with where its bytes come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OfferContent {
    Text(String),
    File(PathBuf),
}

/// The MIME types (most specific first) a drag of `items` offers. `blob`
/// resolves a stash file's path. Files and stash pictures are listed as
/// `text/uri-list`; a lone picture also offers its bytes (`image/png` …);
/// a lone text its text (and HTML); links their URLs (and Firefox's
/// `text/x-moz-url`); everything also as `text/plain`.
pub fn drag_offers(items: &[&Item], blob: impl Fn(&str) -> PathBuf) -> Vec<(String, OfferContent)> {
    let mut out: Vec<(String, OfferContent)> = Vec::new();
    let mut uris = String::new();
    let mut plain: Vec<String> = Vec::new();
    let mut moz = String::new();
    for item in items {
        match &item.kind {
            ItemKind::File { path, .. } => {
                uris.push_str(&file_uri(path));
                uris.push_str("\r\n");
                plain.push(path.display().to_string());
            }
            ItemKind::Image { blob: b, .. } | ItemKind::Data { blob: b, .. } => {
                let p = blob(b);
                uris.push_str(&file_uri(&p));
                uris.push_str("\r\n");
                plain.push(p.display().to_string());
            }
            ItemKind::Url { url, title, .. } => {
                uris.push_str(url);
                uris.push_str("\r\n");
                plain.push(url.clone());
                moz.push_str(url);
                moz.push('\n');
                moz.push_str(title.as_deref().unwrap_or(url));
                moz.push('\n');
            }
            ItemKind::Text { .. } => {}
        }
    }
    if let [only] = items {
        match &only.kind {
            ItemKind::Image { blob: b, mime, .. } => out.push((mime.clone(), OfferContent::File(blob(b)))),
            ItemKind::Text { blob: b, html, .. } => {
                out.push((TEXT_UTF8.into(), OfferContent::File(blob(b))));
                if let Some(h) = html {
                    out.push((HTML.into(), OfferContent::File(blob(h))));
                }
                out.push((TEXT_PLAIN.into(), OfferContent::File(blob(b))));
                out.push((UTF8_STRING.into(), OfferContent::File(blob(b))));
                return out;
            }
            ItemKind::Data { blob: b, mime, .. } => out.push((mime.clone(), OfferContent::File(blob(b)))),
            _ => {}
        }
    }
    if !moz.is_empty() && items.iter().all(|i| matches!(i.kind, ItemKind::Url { .. })) {
        out.push((MOZ_URL.into(), OfferContent::Text(moz.trim_end().to_string())));
    }
    if !uris.is_empty() {
        out.push((URI_LIST.into(), OfferContent::Text(uris)));
    }
    if !plain.is_empty() {
        let text = plain.join("\n");
        out.push((TEXT_UTF8.into(), OfferContent::Text(text.clone())));
        out.push((TEXT_PLAIN.into(), OfferContent::Text(text)));
    }
    out
}

// ── The cache limit ──────────────────────────────────────────────────────

/// What the cache limit decides.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Eviction {
    /// Closed shelves whose folders go, oldest first.
    pub evict: Vec<u64>,
    /// The open shelves alone are over the limit: ask, never delete.
    pub over_with_open: bool,
}

/// Plans eviction under `limit` bytes: `open` is what open shelves hold,
/// `closed` the closed shelves `(id, bytes, closed_at)` and `orphans` the
/// shelves' folders that belong to no shelf (always removed).
pub fn plan_eviction(open: u64, closed: &[(u64, u64, u64)], limit: u64) -> Eviction {
    let mut total = open + closed.iter().map(|c| c.1).sum::<u64>();
    let mut order: Vec<&(u64, u64, u64)> = closed.iter().collect();
    order.sort_by_key(|c| (c.2, c.0));
    let mut evict = Vec::new();
    for c in order {
        if total <= limit {
            break;
        }
        evict.push(c.0);
        total -= c.1;
    }
    Eviction { evict, over_with_open: open > limit }
}

// ── Command targets ──────────────────────────────────────────────────────

/// Which shelves a command acts on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Target {
    One(u64),
    All,
    /// The most recent shelf.
    Latest,
}

/// Parses `all`, `last` / `latest` (or nothing) and a shelf id (`3` or `#3`).
pub fn parse_target(arg: Option<&str>) -> Result<Target, String> {
    match arg.map(str::trim) {
        None | Some("") | Some("last") | Some("latest") => Ok(Target::Latest),
        Some("all") => Ok(Target::All),
        Some(s) => s.trim_start_matches('#').parse::<u64>().ok().filter(|id| *id > 0).map(Target::One).ok_or_else(|| mandara_i18n::tf("Not a shelf: {arg} (use a shelf id from `stash list`, `last` or `all`)", &[("arg", &s)])),
    }
}

/// The shelf ids a target picks in `state`.
pub fn resolve_target(state: &StashState, target: Target) -> Vec<u64> {
    match target {
        Target::All => state.shelves.iter().map(|s| s.id).collect(),
        Target::Latest => state.latest().into_iter().collect(),
        Target::One(id) => state.shelf(id).map(|s| s.id).into_iter().collect(),
    }
}

/// What `stash add` adds: a path, a link, or text (`text:` prefix).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddArg {
    Path(PathBuf),
    Link(String),
    Text(String),
}

pub fn parse_add(arg: &str) -> AddArg {
    if let Some(text) = arg.strip_prefix("text:") {
        return AddArg::Text(text.to_string());
    }
    if let Some(path) = file_uri_path(arg) {
        return AddArg::Path(path);
    }
    if single_link(arg).is_some() {
        return AddArg::Link(arg.trim().to_string());
    }
    AddArg::Path(mandara_config::paths::expand_tilde(arg))
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
