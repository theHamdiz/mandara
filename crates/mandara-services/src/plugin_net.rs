//! HTTPS for third-party plugins granted `net:<host>`: one request to
//! exactly that host, bounded in size and time, never redirected and never
//! to an address on your own machine or network. Runs on a background
//! thread (the caller's), never on the UI thread.

use std::io::Read as _;
use std::net::{IpAddr, ToSocketAddrs as _};
use std::time::Duration;

use ureq::http::{Method, Request, header};

/// A plugin request may take this long in all.
pub const TIMEOUT: Duration = Duration::from_secs(10);
/// Largest request body a plugin may send.
pub const MAX_BODY_BYTES: usize = 16 * 1024;
/// Largest response a plugin receives (it also has its reply limit).
pub const MAX_RESPONSE_BYTES: usize = 256 * 1024;
/// HTTPS's port, the only one allowed.
const HTTPS_PORT: u16 = 443;

/// The answer: HTTP status and the body as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

/// Checks `url` against the granted `host`: `https://<host>[/path][?query]`,
/// no credentials, no other port, no fragment.
pub fn check_url(url: &str, host: &str) -> Result<ureq::http::Uri, String> {
    let uri: ureq::http::Uri = url.parse().map_err(|_| format!("`{url}` isn't a URL"))?;
    if uri.scheme_str() != Some("https") {
        return Err("only https:// addresses are allowed".into());
    }
    let authority = uri.authority().ok_or("the URL has no host")?;
    if authority.as_str().contains('@') {
        return Err("addresses with a user name or password aren't allowed".into());
    }
    if !authority.host().eq_ignore_ascii_case(host) {
        return Err(format!("this plugin may only connect to {host}"));
    }
    if authority.port_u16().is_some_and(|p| p != HTTPS_PORT) {
        return Err("only the standard HTTPS port is allowed".into());
    }
    if url.contains('#') {
        return Err("fragments aren't sent to servers".into());
    }
    Ok(uri)
}

/// Whether `ip` is on this machine, a private, link-local, shared,
/// benchmarking, multicast or reserved network, or a translation prefix
/// that could reach one: anything but the public internet.
pub fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified() || v4.is_broadcast() || v4.is_documentation() || v4.is_multicast()
                // 100.64/10 shared, 198.18/15 benchmarking, 240/4 reserved, 0/8.
                || (a == 100 && (b & 0xc0) == 64) || (a == 198 && (b & 0xfe) == 18) || a >= 240 || a == 0
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            v6.is_loopback() || v6.is_unspecified() || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // unique local
                || (s[0] & 0xffc0) == 0xfe80 // link-local
                || (s[0] & 0xffc0) == 0xfec0 // site-local (deprecated)
                || s[0] == 0x2002 // 6to4: wraps any IPv4 address
                || (s[0] == 0x64 && s[1] == 0xff9b) // NAT64: wraps any IPv4 address
                || (s[0] == 0x2001 && s[1] == 0x0db8) // documentation
                || v6.to_ipv4_mapped().is_some_and(|v4| is_local(IpAddr::V4(v4)))
                || v6.to_ipv4().is_some_and(|v4| s[..6] == [0; 6] && is_local(IpAddr::V4(v4)))
        }
    }
}

/// Hands ureq the addresses already checked by [`fetch`], so the connection
/// goes exactly where the check looked (a second lookup could be answered
/// differently: DNS rebinding).
/// Addresses ureq keeps from a resolver.
const PINNED_MAX: usize = 16;

#[derive(Debug)]
struct Pinned(Vec<std::net::SocketAddr>);

impl ureq::unversioned::resolver::Resolver for Pinned {
    fn resolve(&self, _uri: &ureq::http::Uri, _config: &ureq::config::Config, _timeout: ureq::unversioned::transport::NextTimeout) -> Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
        let mut out = self.empty();
        for a in self.0.iter().take(PINNED_MAX) {
            out.push(*a);
        }
        if out.is_empty() { Err(ureq::Error::HostNotFound) } else { Ok(out) }
    }
}

/// Sends one request. `method` is GET or POST (`body` only with POST).
/// The host's addresses are looked up first and refused when any is local,
/// so a public name can't be pointed at your router or printer.
pub fn fetch(host: &str, url: &str, method: &str, body: Option<&str>) -> Result<Response, String> {
    crate::offline::check()?;
    let uri = check_url(url, host)?;
    let method = match method {
        "" | "GET" | "get" => Method::GET,
        "POST" | "post" => Method::POST,
        other => return Err(format!("method {other} isn't allowed (GET or POST)")),
    };
    let body = body.unwrap_or_default();
    if body.len() > MAX_BODY_BYTES {
        return Err(format!("the request body is larger than {MAX_BODY_BYTES} bytes"));
    }
    if method == Method::GET && !body.is_empty() {
        return Err("a GET request has no body".into());
    }
    let addresses = (host, HTTPS_PORT).to_socket_addrs().map_err(|e| format!("{host}: {e}"))?.collect::<Vec<_>>();
    if addresses.is_empty() || addresses.iter().any(|a| is_local(a.ip())) {
        return Err(format!("{host} points at a local or private address"));
    }
    let config = ureq::Agent::config_builder().timeout_global(Some(TIMEOUT)).max_redirects(0).http_status_as_error(false).build();
    let agent = ureq::Agent::with_parts(config, ureq::unversioned::transport::DefaultConnector::default(), Pinned(addresses));
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::USER_AGENT, concat!("mandara-shell-plugin/", env!("CARGO_PKG_VERSION")))
        .body(body.as_bytes().to_vec())
        .map_err(|e| e.to_string())?;
    let mut response = agent.run(request).map_err(|e| format!("request failed: {e}"))?;
    let status = response.status().as_u16();
    let mut bytes = Vec::new();
    response.body_mut().as_reader().take((MAX_RESPONSE_BYTES + 1) as u64).read_to_end(&mut bytes).map_err(|e| format!("request failed: {e}"))?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(format!("the response was larger than {MAX_RESPONSE_BYTES} bytes"));
    }
    let body = String::from_utf8(bytes).map_err(|_| "the response isn't text".to_string())?;
    Ok(Response { status, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_must_name_the_granted_host_over_https() {
        assert!(check_url("https://api.example.com/v1/moon?x=1", "api.example.com").is_ok());
        assert!(check_url("https://API.example.com/", "api.example.com").is_ok());
        for bad in ["http://api.example.com/", "https://evil.com/", "https://api.example.com.evil.com/", "https://user:pw@api.example.com/", "https://api.example.com:8443/", "https://api.example.com/#frag", "ftp://api.example.com", "nonsense"] {
            assert!(check_url(bad, "api.example.com").is_err(), "{bad}");
        }
    }

    #[test]
    fn local_and_private_addresses_are_refused() {
        for local in ["127.0.0.1", "10.1.2.3", "192.168.1.1", "172.16.0.1", "169.254.1.1", "100.64.0.1", "0.0.0.0", "198.18.0.1", "224.0.0.1", "240.0.0.1", "::1", "fe80::1", "fd00::1", "::ffff:192.168.1.1", "64:ff9b::a00:1", "2002:c0a8:101::1", "ff02::1", "::127.0.0.1"] {
            assert!(is_local(local.parse().unwrap()), "{local}");
        }
        for public in ["1.1.1.1", "93.184.216.34", "2606:4700:4700::1111"] {
            assert!(!is_local(public.parse().unwrap()), "{public}");
        }
    }

    #[test]
    fn methods_and_bodies_are_bounded_before_any_network_io() {
        assert!(fetch("api.example.com", "https://api.example.com/", "DELETE", None).unwrap_err().contains("DELETE"));
        assert!(fetch("api.example.com", "https://api.example.com/", "GET", Some("x")).unwrap_err().contains("no body"));
        assert!(fetch("api.example.com", "https://api.example.com/", "POST", Some(&"x".repeat(MAX_BODY_BYTES + 1))).unwrap_err().contains("larger"));
        assert!(fetch("api.example.com", "https://localhost/", "GET", None).unwrap_err().contains("only connect"));
    }
}
