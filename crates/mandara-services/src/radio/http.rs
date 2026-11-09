//! A small HTTP/1.0 client for audio streams: plain TCP or TLS (rustls with
//! the bundled web PKI roots, the stack `ureq` already links), redirects,
//! the `ICY 200 OK` status line of older servers, `icy-*` headers, and a
//! short socket read timeout so the network thread can notice a stop or a
//! stalled server at once. HTTP/1.0 keeps responses unchunked.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mandara_i18n::{t, tf};

/// Redirects followed before giving up.
pub const MAX_REDIRECTS: usize = 5;
/// Largest response head accepted (bytes).
pub const MAX_HEAD: usize = 16 * 1024;
/// How long connecting (each resolved address) may take.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
/// How long the server may take to answer.
pub const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);
/// Socket read timeout: how often a blocked read looks at the stop flag.
pub const READ_POLL: Duration = Duration::from_millis(250);
/// Header values kept (characters).
const MAX_VALUE_CHARS: usize = 120;

pub const USER_AGENT: &str = concat!("mandara-shell/", env!("CARGO_PKG_VERSION"), " (https://github.com/theHamdiz/mandara)");

/// What the server said about the stream.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Head {
    pub status: u16,
    /// Lower-case MIME type without parameters.
    pub content_type: Option<String>,
    pub metaint: Option<usize>,
    pub kbps: Option<u32>,
    pub name: Option<String>,
    pub genre: Option<String>,
    pub location: Option<String>,
    pub chunked: bool,
}

enum Io {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

/// An open response: its head, the final address and the body.
pub struct Response {
    pub head: Head,
    pub url: String,
    io: Io,
    /// Body bytes that arrived with the head.
    early: Vec<u8>,
    early_at: usize,
}

impl Response {
    /// Reads body bytes. `Ok(0)` is the end of the stream; a read that only
    /// timed out returns `ErrorKind::WouldBlock` (try again).
    pub fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.early_at < self.early.len() {
            let n = (self.early.len() - self.early_at).min(out.len());
            out[..n].copy_from_slice(&self.early[self.early_at..self.early_at + n]);
            self.early_at += n;
            if self.early_at == self.early.len() {
                self.early = Vec::new();
                self.early_at = 0;
            }
            return Ok(n);
        }
        let r = match &mut self.io {
            Io::Plain(s) => s.read(out),
            Io::Tls(s) => s.read(out),
        };
        match r {
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => Err(std::io::ErrorKind::WouldBlock.into()),
            // A server that closes without TLS close_notify just ended.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(0),
            other => other,
        }
    }

    /// The first body bytes (at least `want` unless the stream ends or
    /// `deadline` passes) without consuming them, to recognise the codec.
    pub fn peek(&mut self, want: usize, deadline: Instant, cancelled: &dyn Fn() -> bool) -> &[u8] {
        let mut buf = [0u8; 4096];
        while self.early.len() - self.early_at < want && Instant::now() < deadline && !cancelled() {
            let r = match &mut self.io {
                Io::Plain(s) => s.read(&mut buf),
                Io::Tls(s) => s.read(&mut buf),
            };
            match r {
                Ok(0) => break,
                Ok(n) => self.early.extend_from_slice(&buf[..n]),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(_) => break,
            }
        }
        &self.early[self.early_at..]
    }

    /// Reads up to `limit` body bytes (playlists), within `deadline`.
    pub fn read_to_limit(&mut self, limit: usize, deadline: Instant, cancelled: &dyn Fn() -> bool) -> Result<Vec<u8>, String> {
        let mut body = Vec::with_capacity(limit.min(8 * 1024));
        let mut buf = [0u8; 4096];
        while body.len() < limit {
            if cancelled() {
                return Err(String::new());
            }
            if Instant::now() > deadline {
                break;
            }
            match self.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => body.extend_from_slice(&buf[..n.min(limit - body.len())]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(body)
    }
}

/// Connection failures, already worded for people.
pub type Error = String;

fn tls_config() -> Result<Arc<rustls::ClientConfig>, Error> {
    let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider).with_safe_default_protocol_versions().map_err(|e| e.to_string())?;
    Ok(Arc::new(builder.with_root_certificates(roots).with_no_client_auth()))
}

struct Target {
    tls: bool,
    host: String,
    port: u16,
    path: String,
}

fn target(url: &str) -> Result<Target, Error> {
    let uri: ureq::http::Uri = url.trim().parse().map_err(|_| t("That doesn't look like a web address").to_string())?;
    let tls = match uri.scheme_str().map(str::to_ascii_lowercase).as_deref() {
        Some("https") => true,
        Some("http") => false,
        _ => return Err(t("Stream addresses start with http:// or https://").into()),
    };
    let host = uri.host().filter(|h| !h.is_empty()).ok_or_else(|| t("The address has no server name").to_string())?;
    let host = host.trim_start_matches('[').trim_end_matches(']').to_string();
    let port = uri.port_u16().unwrap_or(if tls { 443 } else { 80 });
    let path = uri.path_and_query().map(|p| p.as_str()).filter(|p| !p.is_empty()).unwrap_or("/").to_string();
    Ok(Target { tls, host, port, path })
}

fn connect(t_: &Target, cancelled: &dyn Fn() -> bool) -> Result<TcpStream, Error> {
    let addrs = (t_.host.as_str(), t_.port).to_socket_addrs().map_err(|_| tf("Couldn't find the server {host}", &[("host", &t_.host)]))?;
    let mut last = None;
    for addr in addrs.take(4) {
        if cancelled() {
            return Err(String::new());
        }
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(s) => return Ok(s),
            Err(e) => last = Some(e),
        }
    }
    Err(match last {
        Some(e) if e.kind() == std::io::ErrorKind::TimedOut => tf("{host} didn't answer", &[("host", &t_.host)]),
        Some(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => tf("{host} refused the connection", &[("host", &t_.host)]),
        _ => tf("Couldn't connect to {host}", &[("host", &t_.host)]),
    })
}

fn value(raw: &[u8]) -> String {
    let s = String::from_utf8_lossy(raw);
    let s = s.trim();
    match s.char_indices().nth(MAX_VALUE_CHARS) {
        Some((end, _)) => s[..end].to_string(),
        None => s.to_string(),
    }
}

/// Parses a response head (`HTTP/1.x` or `ICY`); `None` while incomplete.
pub fn parse_head(raw: &[u8]) -> Result<Option<(Head, usize)>, Error> {
    // httparse only knows HTTP: treat "ICY 200 OK" as HTTP/1.0.
    let (bytes, shift): (std::borrow::Cow<'_, [u8]>, usize) = if raw.starts_with(b"ICY ") {
        let mut v = Vec::with_capacity(raw.len() + 5);
        v.extend_from_slice(b"HTTP/1.0 ");
        v.extend_from_slice(&raw[4..]);
        (v.into(), 5)
    } else {
        (raw.into(), 0)
    };
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut resp = httparse::Response::new(&mut headers);
    let len = match resp.parse(&bytes) {
        Ok(httparse::Status::Complete(n)) => n - shift,
        Ok(httparse::Status::Partial) => return Ok(None),
        Err(_) => return Err(t("The server's answer couldn't be read").into()),
    };
    let mut head = Head { status: resp.code.unwrap_or(0), ..Head::default() };
    for h in resp.headers.iter() {
        let name = h.name;
        if name.eq_ignore_ascii_case("content-type") {
            let v = value(h.value).to_ascii_lowercase();
            let mime = v.split(';').next().unwrap_or("").trim().to_string();
            head.content_type = (!mime.is_empty()).then_some(mime);
        } else if name.eq_ignore_ascii_case("icy-metaint") {
            head.metaint = value(h.value).parse().ok().filter(|m| *m > 0);
        } else if name.eq_ignore_ascii_case("icy-br") {
            // Some servers list several ("128,128").
            head.kbps = value(h.value).split(',').next().and_then(|v| v.trim().parse().ok()).filter(|k| *k > 0);
        } else if name.eq_ignore_ascii_case("icy-name") {
            head.name = Some(value(h.value)).filter(|v| !v.is_empty() && v != "no name" && v != "Unspecified name");
        } else if name.eq_ignore_ascii_case("icy-genre") {
            head.genre = Some(value(h.value)).filter(|v| !v.is_empty());
        } else if name.eq_ignore_ascii_case("location") {
            head.location = Some(String::from_utf8_lossy(h.value).trim().to_string()).filter(|v| !v.is_empty());
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            head.chunked = value(h.value).to_ascii_lowercase().contains("chunked");
        }
    }
    Ok(Some((head, len)))
}

fn request_bytes(t_: &Target) -> Vec<u8> {
    let host = if (t_.tls && t_.port == 443) || (!t_.tls && t_.port == 80) { t_.host.clone() } else { format!("{}:{}", t_.host, t_.port) };
    format!("GET {} HTTP/1.0\r\nHost: {host}\r\nUser-Agent: {USER_AGENT}\r\nAccept: */*\r\nIcy-MetaData: 1\r\nConnection: close\r\n\r\n", t_.path).into_bytes()
}

fn open_once(url: &str, tls: &mut Option<Arc<rustls::ClientConfig>>, cancelled: &dyn Fn() -> bool) -> Result<Response, Error> {
    let target = target(url)?;
    let tcp = connect(&target, cancelled)?;
    let _ = tcp.set_nodelay(true);
    // The handshake gets the full answer time (far servers take a while);
    // the head and body are then read in short, interruptible steps.
    tcp.set_read_timeout(Some(CONNECT_TIMEOUT)).map_err(|e| e.to_string())?;
    tcp.set_write_timeout(Some(RESPONSE_TIMEOUT)).map_err(|e| e.to_string())?;
    let mut io = if target.tls {
        let config = match tls {
            Some(c) => c.clone(),
            None => tls.insert(tls_config()?).clone(),
        };
        let name = rustls::pki_types::ServerName::try_from(target.host.clone()).map_err(|_| t("The address has no server name").to_string())?;
        let conn = rustls::ClientConnection::new(config, name).map_err(|e| e.to_string())?;
        let mut stream = rustls::StreamOwned::new(conn, tcp);
        while stream.conn.is_handshaking() {
            if cancelled() {
                return Err(String::new());
            }
            stream.conn.complete_io(&mut stream.sock).map_err(|e| tls_message(&e, &target.host))?;
        }
        Io::Tls(Box::new(stream))
    } else {
        Io::Plain(tcp)
    };
    match &io {
        Io::Plain(s) => s.set_read_timeout(Some(READ_POLL)),
        Io::Tls(s) => s.sock.set_read_timeout(Some(READ_POLL)),
    }
    .map_err(|e| e.to_string())?;
    let request = request_bytes(&target);
    let started = Instant::now();
    let write = match &mut io {
        Io::Plain(s) => s.write_all(&request).and_then(|_| s.flush()),
        Io::Tls(s) => s.write_all(&request).and_then(|_| s.flush()),
    };
    write.map_err(|e| tls_message(&e, &target.host))?;
    let mut raw = Vec::with_capacity(2048);
    let mut buf = [0u8; 2048];
    loop {
        if cancelled() {
            return Err(String::new());
        }
        if started.elapsed() > RESPONSE_TIMEOUT {
            return Err(tf("{host} didn't answer", &[("host", &target.host)]));
        }
        let n = match &mut io {
            Io::Plain(s) => s.read(&mut buf),
            Io::Tls(s) => s.read(&mut buf),
        };
        match n {
            Ok(0) => return Err(tf("{host} closed the connection", &[("host", &target.host)])),
            Ok(n) => raw.extend_from_slice(&buf[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
            Err(e) => return Err(tls_message(&e, &target.host)),
        }
        if let Some((head, len)) = parse_head(&raw)? {
            let early = raw.split_off(len);
            return Ok(Response { head, url: url.to_string(), io, early, early_at: 0 });
        }
        if raw.len() > MAX_HEAD {
            return Err(t("The server's answer couldn't be read").into());
        }
    }
}

fn tls_message(e: &std::io::Error, host: &str) -> String {
    let text = e.to_string();
    tracing::debug!("radio: {host}: {text}");
    if text.contains("certificate") || text.contains("Certificate") || text.contains("UnknownIssuer") || text.contains("NotValidForName") {
        tf("{host} has an invalid security certificate", &[("host", &host)])
    } else {
        tf("Couldn't connect to {host}", &[("host", &host)])
    }
}

/// Opens `url`, following redirects. Only `200` answers are returned;
/// `cancelled` is polled while waiting (an empty error means cancelled).
pub fn open(url: &str, cancelled: &dyn Fn() -> bool) -> Result<Response, Error> {
    let mut tls = None;
    let mut url = url.trim().to_string();
    for _ in 0..=MAX_REDIRECTS {
        crate::offline::check()?;
        let resp = open_once(&url, &mut tls, cancelled)?;
        match resp.head.status {
            200 => {
                if resp.head.chunked {
                    return Err(t("This server sends its stream in a way Mandara can't play yet").into());
                }
                return Ok(resp);
            }
            301 | 302 | 303 | 307 | 308 => {
                let next = resp.head.location.as_deref().and_then(|l| super::playlist::resolve(&url, l)).ok_or_else(|| t("The station's address moved without saying where").to_string())?;
                url = next;
            }
            401 | 403 => return Err(t("The station refused to play here (it may be limited to its own country)").into()),
            404 | 410 => return Err(t("The station's address no longer works").into()),
            500 | 502 | 503 | 504 => return Err(t("The station is full or down right now").into()),
            s => return Err(tf("The station answered with error {code}", &[("code", &s)])),
        }
    }
    Err(t("The station redirected too many times").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heads_of_http_and_icy_servers() {
        let raw = b"HTTP/1.0 200 OK\r\nContent-Type: audio/mpeg; charset=x\r\nicy-br:128,128\r\nicy-metaint: 16000\r\nicy-name: Corniche FM\r\nicy-genre:Jazz\r\n\r\nBODY";
        let (head, len) = parse_head(raw).unwrap().unwrap();
        assert_eq!(&raw[len..], b"BODY");
        assert_eq!(head, Head { status: 200, content_type: Some("audio/mpeg".into()), metaint: Some(16000), kbps: Some(128), name: Some("Corniche FM".into()), genre: Some("Jazz".into()), location: None, chunked: false });
        let icy = b"ICY 200 OK\r\nicy-metaint:8192\r\ncontent-type:audio/aacp\r\n\r\n\xff\xf1";
        let (head, len) = parse_head(icy).unwrap().unwrap();
        assert_eq!(head.status, 200);
        assert_eq!(head.content_type.as_deref(), Some("audio/aacp"));
        assert_eq!(&icy[len..], b"\xff\xf1", "ICY body offsets survive the status rewrite");
        assert_eq!(parse_head(b"HTTP/1.1 200 OK\r\nContent-Ty").unwrap(), None, "incomplete");
        assert!(parse_head(b"garbage\r\n\r\n").is_err());
        let redirect = b"HTTP/1.1 302 Found\r\nLocation: https://cdn.example/live\r\nTransfer-Encoding: chunked\r\n\r\n";
        let (head, _) = parse_head(redirect).unwrap().unwrap();
        assert_eq!((head.status, head.location.as_deref(), head.chunked), (302, Some("https://cdn.example/live"), true));
        let unnamed = b"HTTP/1.0 200 OK\r\nicy-name: no name\r\nicy-br: x\r\nicy-metaint: 0\r\n\r\n";
        let (head, _) = parse_head(unnamed).unwrap().unwrap();
        assert_eq!((head.name, head.kbps, head.metaint), (None, None, None));
    }

    #[test]
    fn requests_name_the_host_and_ask_for_metadata() {
        let t_ = target("https://ice1.somafm.com/groovesalad-128-mp3?x=1").unwrap();
        assert!(t_.tls && t_.port == 443 && t_.path == "/groovesalad-128-mp3?x=1");
        let req = String::from_utf8(request_bytes(&t_)).unwrap();
        assert!(req.starts_with("GET /groovesalad-128-mp3?x=1 HTTP/1.0\r\nHost: ice1.somafm.com\r\n"), "{req}");
        assert!(req.contains("Icy-MetaData: 1\r\n") && req.ends_with("\r\n\r\n"));
        let t_ = target("http://[::1]:8000").unwrap();
        assert!(!t_.tls && t_.host == "::1" && t_.port == 8000 && t_.path == "/");
        assert!(String::from_utf8(request_bytes(&t_)).unwrap().contains("Host: ::1:8000\r\n"));
        assert!(target("ftp://x/").is_err());
    }

    #[test]
    fn a_local_stream_opens_through_a_redirect() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            for (i, stream) in listener.incoming().take(2).enumerate() {
                let mut s = stream.unwrap();
                let mut req = [0u8; 1024];
                let _ = s.read(&mut req);
                let reply: Vec<u8> = if i == 0 {
                    b"HTTP/1.0 302 Found\r\nLocation: /live\r\n\r\n".to_vec()
                } else {
                    let mut r = b"ICY 200 OK\r\ncontent-type: audio/mpeg\r\nicy-br: 128\r\n\r\n".to_vec();
                    r.extend(std::iter::repeat_n(0xAAu8, 10_000));
                    r
                };
                s.write_all(&reply).unwrap();
            }
        });
        let mut resp = open(&format!("http://127.0.0.1:{port}/start"), &|| false).unwrap();
        assert_eq!(resp.url, format!("http://127.0.0.1:{port}/live"));
        assert_eq!(resp.head.kbps, Some(128));
        let mut total = 0;
        let mut buf = [0u8; 1500];
        loop {
            match resp.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    assert!(buf[..n].iter().all(|&b| b == 0xAA));
                    total += n;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("{e}"),
            }
        }
        assert_eq!(total, 10_000);
        server.join().unwrap();
    }
}
