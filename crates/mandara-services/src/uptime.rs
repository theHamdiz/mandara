//! Uptime checks: parses the `checks` setting (`Name=target`, comma
//! separated) and probes each target once — an HTTP(S) request (up on a
//! 2xx / 3xx answer, redirects not followed) or a TCP connect — within a
//! timeout, timing how long it took. Probes block; [`probe_all`] runs a
//! round in parallel on scoped threads, so callers run it off the UI thread.

use std::collections::VecDeque;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs as _};
use std::time::{Duration, Instant};

use mandara_i18n::{t, tf};
use serde::Serialize;

/// A probe that hasn't answered by then counts as down.
pub const TIMEOUT: Duration = Duration::from_secs(5);
/// Results kept per check (sparkline, uptime percentage).
pub const HISTORY: usize = 30;
/// An answer slower than this reads as "slow".
pub const SLOW_MS: u32 = 1000;
/// Check interval in seconds (`interval_s`): default and bounds.
pub const DEFAULT_INTERVAL_S: i64 = 60;
pub const INTERVAL_S_RANGE: (i64, i64) = (10, 3600);
/// Checks probed at most (the rest of the list is ignored).
pub const MAX_CHECKS: usize = 24;
/// Longest error text kept per result.
const MAX_REASON_CHARS: usize = 80;
/// HTTP status classes counted as up: 2xx and 3xx.
const UP_STATUS: std::ops::Range<u16> = 200..400;

/// What a check probes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Target {
    Http { url: String },
    Tcp { host: String, port: u16 },
}

impl Target {
    /// The target as written (`https://…`, `host:port`).
    pub fn display(&self) -> String {
        match self {
            Self::Http { url } => url.clone(),
            Self::Tcp { host, port } if host.contains(':') => format!("[{host}]:{port}"),
            Self::Tcp { host, port } => format!("{host}:{port}"),
        }
    }
}

/// One configured check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Check {
    pub name: String,
    pub target: Target,
}

/// The host of an `http(s)://` URL (`None` when there is none).
fn url_host(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = if let Some(v6) = host.strip_prefix('[') { v6.split(']').next()? } else { host.split(':').next()? };
    (!host.is_empty()).then_some(host)
}

/// Parses one target: `http(s)://…` or `host:port`.
pub fn parse_target(s: &str) -> Result<Target, String> {
    let s = s.trim();
    let lower = s.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return match url_host(s) {
            Some(_) if !s.contains(char::is_whitespace) => Ok(Target::Http { url: s.to_string() }),
            _ => Err(tf("{target} isn't a valid URL", &[("target", &s)])),
        };
    }
    if s.contains("://") {
        return Err(tf("{target}: only http://, https:// or host:port can be checked", &[("target", &s)]));
    }
    let (host, port) = s.rsplit_once(':').ok_or_else(|| tf("{target} needs a port (host:port) or http(s)://", &[("target", &s)]))?;
    let bracketed = host.strip_prefix('[').and_then(|h| h.strip_suffix(']'));
    let port: u16 = port.parse().ok().filter(|p| *p > 0).ok_or_else(|| tf("{target} has no valid port", &[("target", &s)]))?;
    // An IPv6 address needs brackets (`[::1]:22`); a bare one is ambiguous.
    let host_ok = match bracketed {
        Some(v6) => v6.parse::<std::net::Ipv6Addr>().is_ok(),
        None => !host.is_empty() && !host.contains([':', '[', ']']) && !host.contains(char::is_whitespace),
    };
    let host = bracketed.unwrap_or(host);
    if !host_ok {
        return Err(tf("{target} has no valid host", &[("target", &s)]));
    }
    Ok(Target::Tcp { host: host.to_string(), port })
}

/// Parses one entry: `target` or `Name=target`.
pub fn parse_check(entry: &str) -> Result<Check, String> {
    let entry = entry.trim();
    // `Name=` only when the `=` comes before any URL scheme (a URL's query
    // may hold `=` too).
    let (name, target) = match entry.split_once('=') {
        Some((name, target)) if !name.contains("://") && !name.contains('/') => (name.trim(), target.trim()),
        _ => ("", entry),
    };
    let target = parse_target(target)?;
    let name = if !name.is_empty() {
        name.to_string()
    } else {
        match &target {
            Target::Http { url } => url_host(url).unwrap_or(url).to_string(),
            Target::Tcp { .. } => target.display(),
        }
    };
    Ok(Check { name, target })
}

/// Parses the `checks` setting: valid checks, and why the others were
/// skipped.
pub fn parse_checks(setting: &str) -> (Vec<Check>, Vec<String>) {
    let mut checks = Vec::new();
    let mut problems = Vec::new();
    for entry in setting.split([',', '\n']).map(str::trim).filter(|e| !e.is_empty()) {
        match parse_check(entry) {
            Ok(_) if checks.len() >= MAX_CHECKS => {
                problems.push(tf("Only the first {count} checks run", &[("count", &MAX_CHECKS)]));
                break;
            }
            Ok(c) => checks.push(c),
            Err(e) => problems.push(e),
        }
    }
    (checks, problems)
}

/// The result of one probe.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Probe {
    pub up: bool,
    /// Time to answer (also for a refused or failing answer).
    pub ms: Option<u32>,
    /// Why it's down (empty when up).
    pub reason: String,
    /// Unix seconds.
    pub at: i64,
}

fn short(reason: impl std::fmt::Display) -> String {
    let s = reason.to_string();
    let first = s.lines().next().unwrap_or_default();
    if first.chars().count() > MAX_REASON_CHARS { format!("{}…", first.chars().take(MAX_REASON_CHARS).collect::<String>()) } else { first.to_string() }
}

fn millis(d: Duration) -> u32 {
    u32::try_from(d.as_millis()).unwrap_or(u32::MAX)
}

fn http(url: &str, timeout: Duration) -> Result<u32, (Option<u32>, String)> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(timeout)).http_status_as_error(false).max_redirects(0).build().into();
    let started = Instant::now();
    match agent.get(url).header("User-Agent", "Mandara uptime").call() {
        Ok(reply) => {
            let ms = millis(started.elapsed());
            let code = reply.status().as_u16();
            if UP_STATUS.contains(&code) { Ok(ms) } else { Err((Some(ms), tf("HTTP {code}", &[("code", &code)]))) }
        }
        Err(ureq::Error::Timeout(_)) => Err((None, t("timed out").into())),
        Err(e) => Err((None, short(e))),
    }
}

fn tcp(host: &str, port: u16, timeout: Duration) -> Result<u32, (Option<u32>, String)> {
    let started = Instant::now();
    let addrs: Vec<SocketAddr> = (host, port).to_socket_addrs().map_err(|e| (None, short(e)))?.collect();
    if addrs.is_empty() {
        return Err((None, tf("{host} has no address", &[("host", &host)])));
    }
    let mut last = String::new();
    for addr in addrs {
        let left = timeout.saturating_sub(started.elapsed());
        if left.is_zero() {
            return Err((None, t("timed out").into()));
        }
        match TcpStream::connect_timeout(&addr, left) {
            Ok(_) => return Ok(millis(started.elapsed())),
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => last = t("timed out").into(),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => last = t("connection refused").into(),
            Err(e) => last = short(e),
        }
    }
    Err((None, last))
}

/// Probes `target` once (blocking, at most about `timeout`).
pub fn probe(target: &Target, timeout: Duration) -> Probe {
    let result = match target {
        Target::Http { url } => http(url, timeout),
        Target::Tcp { host, port } => tcp(host, *port, timeout),
    };
    let at = chrono::Utc::now().timestamp();
    match result {
        Ok(ms) => Probe { up: true, ms: Some(ms), reason: String::new(), at },
        Err((ms, reason)) => Probe { up: false, ms, reason, at },
    }
}

/// Probes every target at once (one scoped thread each).
pub fn probe_all(targets: &[Target], timeout: Duration) -> Vec<Probe> {
    std::thread::scope(|s| {
        let handles: Vec<_> = targets.iter().map(|t| s.spawn(move || probe(t, timeout))).collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|_| Probe { up: false, ms: None, reason: t("the check crashed").into(), at: chrono::Utc::now().timestamp() }))
            .collect()
    })
}

/// The last [`HISTORY`] results of one check, oldest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Results(VecDeque<Probe>);

impl Results {
    pub fn push(&mut self, p: Probe) {
        if self.0.len() == HISTORY {
            self.0.pop_front();
        }
        self.0.push_back(p);
    }

    pub fn last(&self) -> Option<&Probe> {
        self.0.back()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Probe> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Share of kept results that were up, `0..=1` (`None` before any).
    pub fn uptime(&self) -> Option<f32> {
        (!self.0.is_empty()).then(|| self.0.iter().filter(|p| p.up).count() as f32 / self.0.len() as f32)
    }

    /// Slowest answer kept (for scaling a sparkline).
    pub fn peak_ms(&self) -> u32 {
        self.0.iter().filter_map(|p| p.ms).max().unwrap_or(0)
    }
}

/// The state a check is in after `p`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Health {
    Up,
    Slow,
    Down,
}

impl Health {
    pub fn of(p: &Probe) -> Self {
        match (p.up, p.ms) {
            (false, _) => Self::Down,
            (true, Some(ms)) if ms > SLOW_MS => Self::Slow,
            (true, _) => Self::Up,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tcp_target(host: &str, port: u16) -> Target {
        Target::Tcp { host: host.into(), port }
    }

    #[test]
    fn parses_named_and_plain_targets() {
        let (checks, problems) = parse_checks("Home NAS=nas.local:22, Blog=https://example.com, http://router.lan/status?x=1, 10.0.0.5:5432");
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(checks.len(), 4);
        assert_eq!(checks[0], Check { name: "Home NAS".into(), target: tcp_target("nas.local", 22) });
        assert_eq!(checks[1], Check { name: "Blog".into(), target: Target::Http { url: "https://example.com".into() } });
        assert_eq!(checks[2].name, "router.lan", "an unnamed URL is named after its host; the query's = isn't a name");
        assert_eq!(checks[2].target, Target::Http { url: "http://router.lan/status?x=1".into() });
        assert_eq!(checks[3].name, "10.0.0.5:5432");
    }

    #[test]
    fn handles_ipv6_and_rejects_nonsense() {
        assert_eq!(parse_target("[::1]:8080").unwrap(), tcp_target("::1", 8080));
        assert_eq!(tcp_target("::1", 8080).display(), "[::1]:8080");
        assert_eq!(url_host("https://user@[fe80::1]:8443/x"), Some("fe80::1"));
        assert!(parse_target("nas.local").is_err(), "no port");
        assert!(parse_target("nas.local:0").is_err());
        assert!(parse_target("nas.local:99999").is_err());
        assert!(parse_target("ftp://example.com").is_err());
        assert!(parse_target("https://").is_err());
        assert!(parse_target("::1:22").is_err(), "bare IPv6 needs brackets");
        let (checks, problems) = parse_checks(" , Broken=nas, ok.lan:80 ,");
        assert_eq!(checks.len(), 1);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn caps_the_number_of_checks() {
        let many = (1..=MAX_CHECKS + 3).map(|i| format!("h{i}.lan:22")).collect::<Vec<_>>().join(",");
        let (checks, problems) = parse_checks(&many);
        assert_eq!(checks.len(), MAX_CHECKS);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn results_keep_the_last_ones_and_score_uptime() {
        let mut r = Results::default();
        assert_eq!(r.uptime(), None);
        for i in 0..HISTORY + 5 {
            r.push(Probe { up: i % 4 != 0, ms: Some(i as u32), reason: String::new(), at: i as i64 });
        }
        assert_eq!(r.len(), HISTORY);
        assert_eq!(r.iter().next().unwrap().at, 5);
        let up = r.iter().filter(|p| p.up).count() as f32 / HISTORY as f32;
        assert_eq!(r.uptime(), Some(up));
        assert_eq!(r.peak_ms(), (HISTORY + 4) as u32);
        assert_eq!(Health::of(&Probe { up: true, ms: Some(SLOW_MS + 1), reason: String::new(), at: 0 }), Health::Slow);
        assert_eq!(Health::of(&Probe { up: false, ms: Some(3), reason: String::new(), at: 0 }), Health::Down);
    }

    #[test]
    fn probes_a_local_listener() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let p = probe(&tcp_target("127.0.0.1", port), TIMEOUT);
        assert!(p.up, "{p:?}");
        drop(listener);
        let p = probe(&tcp_target("127.0.0.1", port), TIMEOUT);
        assert!(!p.up);
    }
}
