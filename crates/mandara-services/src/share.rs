//! Share to phone: serves exactly one item (a text, an image or a file)
//! over the local network at an unguessable URL, so a phone can open it
//! from a QR code.
//!
//! A tiny HTTP/1.1 server runs on a background thread, bound to the LAN
//! IPv4 of the primary interface (all interfaces when that bind fails) on a
//! random free port. The only path it answers is `/<token>`, where the
//! token is 22 characters from the OS random generator (about 131 bits);
//! everything else gets a 404, nothing is ever listed. The server stops by
//! itself at its deadline, after the first complete download when asked to
//! (`once`), or when [`Share::stop`] is called or the [`Share`] is dropped.

use std::io::{BufRead as _, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mandara_i18n::{t, tf};
use serde::Serialize;

/// Length of the URL token.
pub const TOKEN_LEN: usize = 22;
/// Token characters (URL-safe without escaping).
const ALPHABET: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
/// Random bytes at or above this are rejected so every character is equally
/// likely (`62 × 4`).
const UNBIASED_BELOW: u8 = 248;
/// Default lifetime of a share (minutes) and its bounds.
pub const DEFAULT_MINUTES: i64 = 10;
pub const MINUTES_RANGE: (i64, i64) = (1, 120);
/// How often the accept loop checks for stop / expiry.
const ACCEPT_POLL: Duration = Duration::from_millis(100);
/// A client that sends nothing for this long is dropped.
const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// A stalled download is dropped after this long.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest request head read.
const MAX_HEAD: usize = 8 * 1024;
/// Connections served at once.
const MAX_CONNECTIONS: usize = 8;
/// Bytes per write while streaming a file.
const CHUNK: usize = 64 * 1024;
/// Largest clipboard image / text taken.
const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TEXT_BYTES: u64 = 1024 * 1024;
/// Characters of a text shown as its preview.
pub const PREVIEW_CHARS: usize = 160;
/// A documentation-only address (TEST-NET-1): connecting a UDP socket to
/// it picks the primary interface's source address without sending anything.
const ROUTE_PROBE: (Ipv4Addr, u16) = (Ipv4Addr::new(192, 0, 2, 1), 9);
/// Tailscale and other CGNAT ranges (`100.64.0.0/10`) aren't the LAN.
const CGNAT: (u8, u8, u8) = (100, 64, 0xC0);

/// What is shared.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Text,
    Image,
    File,
}

#[derive(Clone, Debug)]
enum Body {
    Text(Arc<str>),
    Bytes(Arc<[u8]>),
    File(PathBuf),
}

/// The one thing a share serves.
#[derive(Clone, Debug)]
pub struct Item {
    pub kind: Kind,
    /// File name offered to the phone (or a title for a text).
    pub name: String,
    pub mime: String,
    /// Bytes (characters' UTF-8 bytes for a text).
    pub size: u64,
    /// The start of a text.
    pub preview: Option<String>,
    /// Where an image or file lives on disk.
    pub path: Option<PathBuf>,
    body: Body,
}

impl Item {
    /// A text, served as a small page with a copy button.
    pub fn text(text: &str) -> Result<Self, String> {
        if text.trim().is_empty() {
            return Err(t("There is no text to share").into());
        }
        let preview: String = text.trim().chars().take(PREVIEW_CHARS).collect();
        Ok(Self { kind: Kind::Text, name: t("Text").into(), mime: "text/plain".into(), size: text.len() as u64, preview: Some(preview), path: None, body: Body::Text(text.into()) })
    }

    /// Image bytes held in memory (a clipboard image).
    pub fn image_bytes(bytes: Vec<u8>, mime: &str) -> Self {
        let ext = extension_for(mime);
        Self { kind: Kind::Image, name: format!("clipboard.{ext}"), mime: mime.into(), size: bytes.len() as u64, preview: None, path: None, body: Body::Bytes(bytes.into()) }
    }

    /// A regular file on disk (never a directory).
    pub fn file(path: &Path) -> Result<Self, String> {
        let path = std::fs::canonicalize(path).map_err(|e| tf("{path}: {error}", &[("path", &path.display()), ("error", &e)]))?;
        let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
        if !meta.is_file() {
            return Err(tf("{path} isn't a file (folders are never shared)", &[("path", &path.display())]));
        }
        std::fs::File::open(&path).map_err(|e| tf("Can't read {path}: {error}", &[("path", &path.display()), ("error", &e)]))?;
        let mime = mime_for(&path);
        let kind = if mime.starts_with("image/") { Kind::Image } else { Kind::File };
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
        Ok(Self { kind, name, mime: mime.into(), size: meta.len(), preview: None, path: Some(path.clone()), body: Body::File(path) })
    }

    /// Image bytes to show as a thumbnail (in-memory images only; files are
    /// read from `path`).
    pub fn image_data(&self) -> Option<Arc<[u8]>> {
        match &self.body {
            Body::Bytes(b) if self.kind == Kind::Image => Some(b.clone()),
            _ => None,
        }
    }
}

/// MIME type by file extension (`application/octet-stream` when unknown).
pub fn mime_for(path: &Path) -> &'static str {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "avif" => "image/avif",
        "heic" => "image/heic",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "pdf" => "application/pdf",
        "txt" | "md" | "log" => "text/plain; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "json" => "application/json",
        "zip" => "application/zip",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "wav" => "audio/wav",
        "epub" => "application/epub+zip",
        "apk" => "application/vnd.android.package-archive",
        _ => "application/octet-stream",
    }
}

fn extension_for(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/avif" => "avif",
        "image/bmp" => "bmp",
        "image/svg+xml" => "svg",
        _ => "png",
    }
}

/// `count` token characters from a stream of random bytes (rejection
/// sampling keeps every character equally likely).
fn token_from(bytes: impl IntoIterator<Item = u8>, count: usize) -> String {
    bytes.into_iter().filter(|b| *b < UNBIASED_BELOW).take(count).map(|b| ALPHABET[usize::from(b) % ALPHABET.len()] as char).collect()
}

/// A fresh [`TOKEN_LEN`]-character token from the OS random generator.
pub fn token() -> Result<String, String> {
    let mut out = String::new();
    while out.len() < TOKEN_LEN {
        let mut buf = [0u8; TOKEN_LEN * 2];
        getrandom::fill(&mut buf).map_err(|e| tf("No randomness available: {error}", &[("error", &e)]))?;
        out.push_str(&token_from(buf, TOKEN_LEN - out.len()));
    }
    Ok(out)
}

/// `http://<ip>:<port>/<token>`.
pub fn build_url(ip: IpAddr, port: u16, token: &str) -> String {
    format!("http://{}/{token}", SocketAddr::new(ip, port))
}

/// What a request asks for.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// The shared item (`head`: headers only).
    Item { head: bool },
    NotFound,
    MethodNotAllowed,
    BadRequest,
}

/// Equal-time comparison so the token can't be guessed byte by byte.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Routes a request line (`GET /<token> HTTP/1.1`). Only `/<token>` (with
/// an optional trailing slash or query) serves the item.
pub fn route(request_line: &str, token: &str) -> Route {
    let mut parts = request_line.split(' ');
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next()) else { return Route::BadRequest };
    if parts.next().is_some() || !version.starts_with("HTTP/1.") {
        return Route::BadRequest;
    }
    let path = target.split(['?', '#']).next().unwrap_or_default();
    let path = path.strip_suffix('/').filter(|p| !p.is_empty()).unwrap_or(path);
    let Some(candidate) = path.strip_prefix('/') else { return Route::NotFound };
    if !same(candidate.as_bytes(), token.as_bytes()) {
        return Route::NotFound;
    }
    match method {
        "GET" => Route::Item { head: false },
        "HEAD" => Route::Item { head: true },
        _ => Route::MethodNotAllowed,
    }
}

fn private_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_private() && !(o[0] == CGNAT.0 && (o[1] & CGNAT.2) == CGNAT.1)
}

/// The LAN IPv4 to advertise: the primary interface's address when it is
/// a private LAN address, else another private one, else the primary one.
pub fn lan_ip() -> Option<Ipv4Addr> {
    let routed = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok().and_then(|s| s.connect(ROUTE_PROBE).ok().and_then(|_| s.local_addr().ok())).and_then(|a| match a.ip() {
        IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
        _ => None,
    });
    if let Some(ip) = routed.filter(|ip| private_v4(*ip)) {
        return Some(ip);
    }
    let others: Vec<Ipv4Addr> = crate::net_speed::local_addresses()
        .into_iter()
        .filter_map(|a| match a.addr {
            IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_link_local() => Some(ip),
            _ => None,
        })
        .collect();
    others.iter().copied().find(|ip| private_v4(*ip)).or(routed).or(others.first().copied())
}

/// HTML-escapes `s`.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// A readable page holding `text` with a copy button (no external assets).
pub fn text_page(text: &str) -> String {
    let (title, copy, copied) = (escape(t("Shared from Mandara")), escape(t("Copy")), escape(t("Copied")));
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="referrer" content="no-referrer"><title>{title}</title><style>
:root{{color-scheme:light dark;--bg:#f6f4ef;--fg:#1d1b19;--card:#fff;--accent:#0b6e79}}
@media (prefers-color-scheme:dark){{:root{{--bg:#101418;--fg:#e8e6e1;--card:#1a2026;--accent:#5fd4d0}}}}
body{{margin:0;padding:20px;background:var(--bg);color:var(--fg);font:17px/1.5 system-ui,sans-serif}}
h1{{font-size:15px;font-weight:600;opacity:.7;margin:0 0 12px}}
textarea{{box-sizing:border-box;width:100%;min-height:55vh;padding:14px;border:0;border-radius:16px;background:var(--card);color:inherit;font:inherit;resize:vertical}}
button{{margin-top:14px;width:100%;padding:14px;border:0;border-radius:999px;background:var(--accent);color:#fff;font:600 17px system-ui,sans-serif}}
</style></head><body><h1>{title}</h1><textarea id="t" dir="auto" readonly>{body}</textarea><button id="b" type="button">{copy}</button><script>
document.getElementById("b").onclick=function(){{var t=document.getElementById("t"),b=this,ok=function(){{b.textContent="{copied}"}};if(navigator.clipboard&&window.isSecureContext){{navigator.clipboard.writeText(t.value).then(ok)}}else{{t.focus();t.select();t.setSelectionRange(0,t.value.length);if(document.execCommand("copy")){{ok()}}}}}};
</script></body></html>"#,
        body = escape(text)
    )
}

/// `Content-Disposition` for `name`: an ASCII fallback plus the exact
/// UTF-8 name (RFC 6266 / 5987).
pub fn content_disposition(inline: bool, name: &str) -> String {
    let ascii: String = name.chars().map(|c| if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' { c } else { '_' }).collect();
    let encoded: String = name.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    format!("{}; filename=\"{ascii}\"; filename*=UTF-8''{encoded}", if inline { "inline" } else { "attachment" })
}

/// Why a share ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Ended {
    Expired,
    Downloaded,
    Stopped,
}

/// News from the server thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A complete download (or the text page delivered).
    Downloaded(u32),
    Ended(Ended),
}

struct Shared {
    token: String,
    item: Item,
    once: bool,
    stop: AtomicBool,
    ended: Mutex<Option<Ended>>,
    downloads: AtomicU32,
    active: AtomicUsize,
    events: async_channel::Sender<Event>,
}

impl Shared {
    fn end(&self, why: Ended) {
        let mut ended = self.ended.lock().unwrap_or_else(|e| e.into_inner());
        if ended.is_none() {
            *ended = Some(why);
        }
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// A running share. Dropping it stops the server.
pub struct Share {
    pub url: String,
    pub item: Item,
    pub started: Instant,
    pub deadline: Instant,
    /// Unix seconds of the deadline.
    pub expires_at: i64,
    pub once: bool,
    shared: Arc<Shared>,
    /// Server news; the receiver ends after [`Event::Ended`].
    pub events: async_channel::Receiver<Event>,
}

impl Share {
    pub fn stop(&self) {
        self.shared.end(Ended::Stopped);
    }

    pub fn downloads(&self) -> u32 {
        self.shared.downloads.load(Ordering::Relaxed)
    }

    pub fn is_running(&self) -> bool {
        !self.shared.stop.load(Ordering::Relaxed) && Instant::now() < self.deadline
    }

    pub fn expires_in(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }
}

impl Drop for Share {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Starts serving `item` for `ttl` (stopping after the first complete
/// download when `once`).
pub fn start(item: Item, ttl: Duration, once: bool) -> Result<Share, String> {
    let ip = lan_ip().ok_or_else(|| t("No local network: connect to Wi-Fi or Ethernet first").to_string())?;
    let listener = TcpListener::bind((ip, 0)).or_else(|_| TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))).map_err(|e| tf("Couldn't open a port: {error}", &[("error", &e)]))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let token = token()?;
    let url = build_url(IpAddr::V4(ip), port, &token);
    let (tx, rx) = async_channel::unbounded();
    let shared = Arc::new(Shared { token, item: item.clone(), once, stop: AtomicBool::new(false), ended: Mutex::new(None), downloads: AtomicU32::new(0), active: AtomicUsize::new(0), events: tx });
    let started = Instant::now();
    let deadline = started + ttl;
    let expires_at = chrono::Utc::now().timestamp() + i64::try_from(ttl.as_secs()).unwrap_or(i64::MAX / 2);
    let server = shared.clone();
    std::thread::Builder::new().name("mandara-share".into()).spawn(move || accept_loop(listener, server, deadline)).map_err(|e| e.to_string())?;
    Ok(Share { url, item, started, deadline, expires_at, once, shared, events: rx })
}

fn accept_loop(listener: TcpListener, shared: Arc<Shared>, deadline: Instant) {
    loop {
        if Instant::now() >= deadline {
            shared.end(Ended::Expired);
        }
        if shared.stop.load(Ordering::Relaxed) {
            break;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                if shared.active.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
                    let _ = stream.shutdown(Shutdown::Both);
                    continue;
                }
                shared.active.fetch_add(1, Ordering::Relaxed);
                let s = shared.clone();
                let spawned = std::thread::Builder::new().name("mandara-share-conn".into()).spawn(move || {
                    handle(stream, &s);
                    s.active.fetch_sub(1, Ordering::Relaxed);
                });
                if spawned.is_err() {
                    shared.active.fetch_sub(1, Ordering::Relaxed);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(ACCEPT_POLL),
            Err(_) => std::thread::sleep(ACCEPT_POLL),
        }
    }
    drop(listener);
    let why = shared.ended.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or(Ended::Stopped);
    let _ = shared.events.try_send(Event::Ended(why));
}

/// Reads the request head; `None` when it is too long or the client left.
fn read_head(stream: &TcpStream) -> Option<String> {
    let mut reader = std::io::BufReader::new(stream.take(MAX_HEAD as u64));
    let mut head = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let blank = line == "\r\n" || line == "\n";
        head.push_str(&line);
        if blank {
            return Some(head);
        }
    }
}

fn respond_plain(stream: &mut TcpStream, status: &str) {
    let body = status.split_once(' ').map(|(_, s)| s).unwrap_or(status);
    let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}", body.len());
}

fn handle(mut stream: TcpStream, shared: &Shared) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let Some(head) = read_head(&stream) else { return };
    let line = head.lines().next().unwrap_or_default();
    let head_only = match route(line, &shared.token) {
        Route::Item { head } => head,
        // Anything but reading the item looks like nothing is there.
        Route::NotFound | Route::MethodNotAllowed => return respond_plain(&mut stream, "404 Not Found"),
        Route::BadRequest => return respond_plain(&mut stream, "400 Bad Request"),
    };
    if shared.stop.load(Ordering::Relaxed) {
        return respond_plain(&mut stream, "404 Not Found");
    }
    let item = &shared.item;
    let page;
    let (content_type, length, disposition) = match &item.body {
        Body::Text(text) => {
            page = text_page(text);
            ("text/html; charset=utf-8".to_string(), page.len() as u64, None)
        }
        Body::Bytes(b) => {
            page = String::new();
            (item.mime.clone(), b.len() as u64, Some(content_disposition(true, &item.name)))
        }
        Body::File(path) => {
            page = String::new();
            let Ok(meta) = std::fs::metadata(path) else { return respond_plain(&mut stream, "404 Not Found") };
            (item.mime.clone(), meta.len(), Some(content_disposition(item.kind == Kind::Image, &item.name)))
        }
    };
    let mut headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {length}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nX-Robots-Tag: noindex\r\n"
    );
    if let Some(d) = disposition {
        headers.push_str(&format!("Content-Disposition: {d}\r\n"));
    }
    if matches!(item.body, Body::Text(_)) {
        headers.push_str("Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'\r\n");
    }
    headers.push_str("Connection: close\r\n\r\n");
    if stream.write_all(headers.as_bytes()).is_err() || head_only {
        return;
    }
    let sent = match &item.body {
        Body::Text(_) => stream.write_all(page.as_bytes()).is_ok(),
        Body::Bytes(b) => stream_chunks(&mut stream, &mut &b[..], length, shared),
        Body::File(path) => match std::fs::File::open(path) {
            Ok(f) => stream_chunks(&mut stream, &mut f.take(length), length, shared),
            Err(_) => false,
        },
    };
    if sent && stream.flush().is_ok() {
        let n = shared.downloads.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = shared.events.try_send(Event::Downloaded(n));
        if shared.once {
            shared.end(Ended::Downloaded);
        }
    }
}

/// Copies `length` bytes in chunks, giving up when the share stops.
fn stream_chunks(stream: &mut TcpStream, source: &mut impl Read, length: u64, shared: &Shared) -> bool {
    let mut buf = vec![0u8; CHUNK];
    let mut left = length;
    while left > 0 {
        if shared.stop.load(Ordering::Relaxed) {
            return false;
        }
        let want = usize::try_from(left.min(CHUNK as u64)).unwrap_or(CHUNK);
        let n = match source.read(&mut buf[..want]) {
            Ok(0) | Err(_) => return false,
            Ok(n) => n,
        };
        if stream.write_all(&buf[..n]).is_err() {
            return false;
        }
        left -= n as u64;
    }
    true
}

/// The newest image in the screenshot folder `dir`.
pub fn latest_screenshot(dir: &Path) -> Result<PathBuf, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| tf("Can't read {path}: {error}", &[("path", &dir.display()), ("error", &e)]))?;
    entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| mime_for(&e.path()).starts_with("image/"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max_by_key(|(at, _)| *at)
        .map(|(_, p)| p)
        .ok_or_else(|| tf("No screenshots in {path} yet", &[("path", &dir.display())]))
}

/// A local file named by a `file://` URI (percent-decoded).
fn file_uri_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.trim().strip_prefix("file://")?;
    let path = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(b) = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    use std::os::unix::ffi::OsStringExt as _;
    Some(PathBuf::from(std::ffi::OsString::from_vec(out)))
}

fn read_limited(pipe: impl Read, limit: u64) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    pipe.take(limit + 1).read_to_end(&mut data).map_err(|e| e.to_string())?;
    if data.len() as u64 > limit {
        return Err(t("The clipboard holds too much to share").into());
    }
    Ok(data)
}

/// What the clipboard holds now: an image, a single copied file, or text
/// (blocking). Entries password managers flag as secret are refused.
pub fn clipboard_item() -> Result<Item, String> {
    use wl_clipboard_rs::paste::{ClipboardType, Error as PasteError, MimeType, Seat, get_contents, get_mime_types_ordered};
    let empty = || t("The clipboard is empty").to_string();
    let types = get_mime_types_ordered(ClipboardType::Regular, Seat::Unspecified).map_err(|e| match e {
        PasteError::NoSeats | PasteError::ClipboardEmpty | PasteError::NoMimeType => empty(),
        e => tf("Couldn't read the clipboard: {error}", &[("error", &e)]),
    })?;
    let refs: Vec<&str> = types.iter().map(String::as_str).collect();
    if crate::clipboard::is_secret(&refs) {
        return Err(t("The clipboard holds a password, so it isn't shared").into());
    }
    let read = |mime: MimeType<'_>, limit: u64| -> Result<(Vec<u8>, String), String> {
        let (pipe, mime) = get_contents(ClipboardType::Regular, Seat::Unspecified, mime).map_err(|e| tf("Couldn't read the clipboard: {error}", &[("error", &e)]))?;
        Ok((read_limited(pipe, limit)?, mime))
    };
    let image = refs.iter().find(|m| **m == "image/png").or_else(|| refs.iter().find(|m| m.starts_with("image/") && !m.contains("svg")));
    if let Some(mime) = image {
        let (bytes, mime) = read(MimeType::Specific(mime), MAX_IMAGE_BYTES)?;
        return Ok(Item::image_bytes(bytes, &mime));
    }
    if refs.contains(&"text/uri-list") {
        let (bytes, _) = read(MimeType::Specific("text/uri-list"), MAX_TEXT_BYTES)?;
        let text = String::from_utf8_lossy(&bytes);
        let uris: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
        if let [one] = uris.as_slice()
            && let Some(path) = file_uri_path(one)
        {
            return Item::file(&path);
        }
    }
    let (bytes, _) = read(MimeType::Text, MAX_TEXT_BYTES).map_err(|_| empty())?;
    Item::text(&String::from_utf8_lossy(&bytes))
}

/// A QR code's modules (`true` = dark), row by row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Qr {
    pub size: usize,
    modules: Vec<bool>,
}

impl Qr {
    pub fn new(text: &str) -> Option<Self> {
        let code = qrcodegen::QrCode::encode_text(text, qrcodegen::QrCodeEcc::Medium).ok()?;
        let n = code.size();
        let size = usize::try_from(n).ok()?;
        let modules = (0..n).flat_map(|y| (0..n).map(move |x| (x, y))).map(|(x, y)| code.get_module(x, y)).collect();
        Some(Self { size, modules })
    }

    pub fn dark(&self, x: usize, y: usize) -> bool {
        x < self.size && y < self.size && self.modules[y * self.size + x]
    }

    /// Runs of equal modules in row `y`: `(start, length, dark)`.
    pub fn runs(&self, y: usize) -> Vec<(usize, usize, bool)> {
        let mut out = Vec::new();
        let mut x = 0;
        while x < self.size {
            let on = self.dark(x, y);
            let start = x;
            while x < self.size && self.dark(x, y) == on {
                x += 1;
            }
            out.push((start, x - start, on));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "AbCdEfGhIjKlMnOpQrStUv";

    #[test]
    fn only_the_token_path_serves_the_item() {
        assert_eq!(route(&format!("GET /{TOKEN} HTTP/1.1"), TOKEN), Route::Item { head: false });
        assert_eq!(route(&format!("GET /{TOKEN}/ HTTP/1.1"), TOKEN), Route::Item { head: false });
        assert_eq!(route(&format!("GET /{TOKEN}?utm=qr HTTP/1.0"), TOKEN), Route::Item { head: false });
        assert_eq!(route(&format!("HEAD /{TOKEN} HTTP/1.1"), TOKEN), Route::Item { head: true });
        assert_eq!(route(&format!("POST /{TOKEN} HTTP/1.1"), TOKEN), Route::MethodNotAllowed);
        for path in ["/", "/favicon.ico", "/AbCdEfGhIjKlMnOpQrStU", "/AbCdEfGhIjKlMnOpQrStUvX", "/abcdefghijklmnopqrstuv", &format!("/{TOKEN}/x"), &format!("/../{TOKEN}"), "//", ""] {
            assert_eq!(route(&format!("GET {path} HTTP/1.1"), TOKEN), Route::NotFound, "{path:?}");
        }
        assert_eq!(route("GET", TOKEN), Route::BadRequest);
        assert_eq!(route(&format!("GET /{TOKEN} SPDY/3"), TOKEN), Route::BadRequest);
        assert_eq!(route(&format!("GET /{TOKEN} HTTP/1.1 extra"), TOKEN), Route::BadRequest);
    }

    #[test]
    fn tokens_are_long_unbiased_and_url_safe() {
        let a = token().unwrap();
        let b = token().unwrap();
        assert_eq!(a.len(), TOKEN_LEN);
        assert_ne!(a, b);
        assert!(a.bytes().all(|c| c.is_ascii_alphanumeric()));
        // Bytes ≥ 248 are skipped; 0, 61 and 62 map to A, 9 and A.
        assert_eq!(token_from([0, 255, 61, 248, 62], 3), "A9A");
        assert_eq!(token_from([1, 2], 5), "BC", "stops when the bytes run out");
    }

    #[test]
    fn urls_name_the_lan_address_port_and_token() {
        assert_eq!(build_url(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)), 40123, TOKEN), format!("http://192.168.1.20:40123/{TOKEN}"));
        assert_eq!(build_url("fe80::1".parse().unwrap(), 8080, "t"), "http://[fe80::1]:8080/t");
        assert!(private_v4(Ipv4Addr::new(10, 0, 0, 4)));
        assert!(!private_v4(Ipv4Addr::new(100, 101, 102, 103)), "Tailscale's range isn't the LAN");
        assert!(!private_v4(Ipv4Addr::new(8, 8, 8, 8)));
    }

    #[test]
    fn pages_and_headers_are_escaped() {
        let page = text_page("<script>alert('x')</script> & \"quotes\"");
        assert!(page.contains("&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt; &amp; &quot;quotes&quot;"));
        assert!(!page.contains("<script>alert"));
        assert_eq!(content_disposition(false, "report.pdf"), "attachment; filename=\"report.pdf\"; filename*=UTF-8''report.pdf");
        assert_eq!(content_disposition(true, "صورة \"1\".png"), "inline; filename=\"____ _1_.png\"; filename*=UTF-8''%D8%B5%D9%88%D8%B1%D8%A9%20%221%22.png");
        assert_eq!(file_uri_path("file:///home/me/My%20File.pdf"), Some(PathBuf::from("/home/me/My File.pdf")));
        assert_eq!(file_uri_path("https://x"), None);
    }

    #[test]
    fn items_refuse_folders_and_empty_text() {
        assert!(Item::text("  \n").is_err());
        assert_eq!(Item::text("hello").unwrap().preview.as_deref(), Some("hello"));
        let dir = tempfile::tempdir().unwrap();
        assert!(Item::file(dir.path()).is_err(), "never a directory");
        let shot = dir.path().join("a.png");
        std::fs::write(&shot, b"png").unwrap();
        let item = Item::file(&shot).unwrap();
        assert_eq!((item.kind, item.mime.as_str(), item.size), (Kind::Image, "image/png", 3));
        assert_eq!(latest_screenshot(dir.path()).unwrap(), std::fs::canonicalize(&shot).unwrap_or(shot));
    }

    fn get(addr: &str, path: &str) -> String {
        let mut s = TcpStream::connect(addr).unwrap();
        write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    #[test]
    fn serves_once_then_stops() {
        let Ok(share) = start(Item::text("hi there").unwrap(), Duration::from_secs(30), true) else { return }; // no network in this sandbox
        let rest = share.url.strip_prefix("http://").unwrap();
        let (addr, token) = rest.split_once('/').unwrap();
        assert!(get(addr, "/").starts_with("HTTP/1.1 404"));
        assert!(get(addr, "/nope").starts_with("HTTP/1.1 404"));
        let page = get(addr, &format!("/{token}"));
        assert!(page.starts_with("HTTP/1.1 200") && page.contains("hi there"), "{page}");
        let first = async_channel::Receiver::recv_blocking(&share.events).unwrap();
        assert_eq!(first, Event::Downloaded(1));
        assert_eq!(async_channel::Receiver::recv_blocking(&share.events).unwrap(), Event::Ended(Ended::Downloaded));
        assert_eq!(share.downloads(), 1);
    }

    #[test]
    fn qr_codes_have_runs_covering_each_row() {
        let qr = Qr::new("http://192.168.1.20:40123/AbCdEfGhIjKlMnOpQrStUv").unwrap();
        assert!(qr.size >= 21);
        for y in [0, qr.size / 2, qr.size - 1] {
            assert_eq!(qr.runs(y).iter().map(|r| r.1).sum::<usize>(), qr.size);
        }
        assert!(qr.dark(0, 0), "finder pattern corner");
    }
}
