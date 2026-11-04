//! Bounded JSON requests for opt-in integrations. Authenticated requests
//! never follow redirects or put their credentials in the address.

use std::io::Read as _;
use std::time::Duration;

use mandara_i18n::{t, tf};
use serde_json::Value;
use ureq::http::{HeaderValue, Method, Request, header};

use crate::credentials::Secret;

/// Integration responses are summaries, not unbounded downloads.
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
/// Enough for a translation or entity action, with room for UTF-8 and JSON.
const MAX_REQUEST_BYTES: usize = 64 * 1024;
/// No integration can hold a foreground action open indefinitely.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

pub fn web_url(value: &str) -> bool {
    value.parse::<ureq::http::Uri>().is_ok_and(|uri| {
        uri.scheme_str().is_some_and(|scheme| scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
            && uri.host().is_some_and(|host| !host.is_empty())
            && !uri.authority().is_some_and(|authority| authority.as_str().contains('@'))
    })
}

/// Keep a reverse proxy's path while refusing query strings and fragments
/// in a server base. Each integration supplies its own fixed endpoint.
pub fn endpoint(server: &str, path: &str) -> Result<String, String> {
    let server = server.trim().trim_end_matches('/');
    if !web_url(server) || server.contains(['?', '#']) {
        return Err(t("Use a server address without a password, query or fragment").into());
    }
    Ok(format!("{server}/{path}"))
}

pub enum Authorization<'a> { Bearer(&'a Secret), DeepL(&'a Secret) }

impl Authorization<'_> {
    fn header(&self) -> Result<HeaderValue, String> {
        let (scheme, secret) = match self { Self::Bearer(secret) => ("Bearer", secret), Self::DeepL(secret) => ("DeepL-Auth-Key", secret) };
        let value = zeroize::Zeroizing::new(format!("{scheme} {}", secret.expose()));
        let mut header = HeaderValue::from_str(&value).map_err(|_| t("The API key cannot be used in a request").to_string())?;
        header.set_sensitive(true);
        Ok(header)
    }
}

#[derive(Clone)]
pub struct Client { agent: ureq::Agent }

pub trait Transport {
    fn json(&self, method: Method, url: &str, body: Option<&Value>, auth: Option<Authorization<'_>>) -> Result<Value, String>;
}

impl Default for Client {
    fn default() -> Self {
        Self { agent: ureq::Agent::config_builder().timeout_global(Some(REQUEST_TIMEOUT)).max_redirects(0).http_status_as_error(false).build().into() }
    }
}

impl Transport for Client {
    fn json(&self, method: Method, url: &str, body: Option<&Value>, auth: Option<Authorization<'_>>) -> Result<Value, String> {
        crate::offline::check()?;
        self.send_online(Self::request(method, url, body, auth)?)
    }
}

impl Client {
    fn request(method: Method, url: &str, body: Option<&Value>, auth: Option<Authorization<'_>>) -> Result<Request<Vec<u8>>, String> {
        if !web_url(url) { return Err(t("Use an http:// or https:// server address without a password").into()); }
        let body = body.map(serde_json::to_vec).transpose().map_err(|_| t("Could not prepare the request").to_string())?.unwrap_or_default();
        if body.len() > MAX_REQUEST_BYTES { return Err(t("This request is too large").into()); }
        let mut request = Request::builder().method(method).uri(url)
            .header(header::USER_AGENT, concat!("mandara-shell/", env!("CARGO_PKG_VERSION")))
            .header(header::ACCEPT, "application/json").header(header::CONTENT_TYPE, "application/json");
        if let Some(auth) = auth { request = request.header(header::AUTHORIZATION, auth.header()?); }
        request.body(body).map_err(|_| t("Could not prepare the request").to_string())
    }

    fn send_online(&self, request: Request<Vec<u8>>) -> Result<Value, String> {
        let mut response = self.agent.run(request).map_err(|error| tf("Request failed: {error}", &[("error", &error)]).to_string())?;
        let status = response.status();
        if status.is_redirection() { return Err(t("The server redirected this request; enter its final address in Studio").into()); }
        if !status.is_success() {
            return Err(match status.as_u16() {
                401 | 403 => t("The server rejected the API key").into(),
                429 => t("The server is receiving too many requests; try again later").into(),
                status => tf("Server replied with HTTP {status}", &[("status", &status)]).to_string(),
            });
        }
        let mut bytes = Vec::new();
        response.body_mut().as_reader().take((MAX_RESPONSE_BYTES + 1) as u64).read_to_end(&mut bytes)
            .map_err(|error| tf("Request failed: {error}", &[("error", &error)]).to_string())?;
        if bytes.len() > MAX_RESPONSE_BYTES { return Err(t("The server's response was too large").into()); }
        if bytes.is_empty() { return Ok(Value::Null); }
        serde_json::from_slice(&bytes).map_err(|_| t("The server did not return valid JSON").to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::net::TcpListener;

    use super::*;

    #[test]
    fn request_headers_hide_the_key_and_keep_it_out_of_the_url() {
        let secret = Secret::new("fixture-private-key".into()).unwrap();
        for auth in [Authorization::Bearer(&secret), Authorization::DeepL(&secret)] {
            let request = Client::request(Method::POST, "https://example.org/api", Some(&serde_json::json!({"text":"hello"})), Some(auth)).unwrap();
            assert!(!format!("{:?}", request.headers()).contains(secret.expose()));
            assert!(!request.uri().to_string().contains(secret.expose()));
            assert!(request.headers()[header::AUTHORIZATION].is_sensitive());
        }
    }

    #[test]
    fn invalid_addresses_and_oversized_input_are_rejected_before_io() {
        for url in ["file:///tmp/data", "https://user:password@example.org", "localhost:8123", "https://"] {
            assert!(Client::request(Method::GET, url, None, None).is_err());
        }
        let large = serde_json::json!({"text": "x".repeat(MAX_REQUEST_BYTES)});
        assert!(Client::request(Method::POST, "https://example.org", Some(&large), None).is_err());
        assert_eq!(endpoint("https://example.org/proxy/", "api/states").unwrap(), "https://example.org/proxy/api/states");
        for server in ["https://example.org?key=secret", "https://example.org/#fragment", "file:///tmp"] {
            assert!(endpoint(server, "api/states").is_err());
        }
    }

    #[test]
    fn redirects_never_forward_an_authenticated_request() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/api", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(REQUEST_TIMEOUT)).unwrap();
            let mut buffer = Vec::new();
            let mut chunk = [0; 1024];
            while !buffer.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let n = stream.read(&mut chunk).unwrap();
                assert_ne!(n, 0);
                buffer.extend_from_slice(&chunk[..n]);
            }
            stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        let secret = Secret::new("fixture-key".into()).unwrap();
        let client = Client { agent: ureq::Agent::config_builder().timeout_global(Some(REQUEST_TIMEOUT)).max_redirects(0).http_status_as_error(false).proxy(None).build().into() };
        let request = Client::request(Method::GET, &url, None, Some(Authorization::Bearer(&secret))).unwrap();
        assert!(client.send_online(request).unwrap_err().contains("redirected"));
        server.join().unwrap();
    }
}
