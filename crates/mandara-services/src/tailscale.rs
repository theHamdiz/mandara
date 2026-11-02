//! Tailscale through its own CLI: `tailscale status --json` for this
//! device, the peers and the exit node in use, plus `tailscale up`,
//! `tailscale down` and `tailscale set --exit-node=…`. Everything here
//! blocks (a child process with a deadline), so callers run it off the UI
//! thread.
//!
//! `tailscale up` on a logged-out device prints a login link and waits for
//! it; [`up`] returns that link as soon as it appears and leaves the CLI
//! running in the background (adopted by [`crate::reaper`]) so the login
//! completes when the browser finishes.

use std::io::{BufRead as _, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use mandara_i18n::{t, tf};
use serde::{Deserialize, Serialize};

/// The CLI.
const BIN: &str = "tailscale";
/// `tailscale status --json` normally answers in milliseconds.
pub const STATUS_TIMEOUT: Duration = Duration::from_secs(8);
/// `up` / `down` / `set` reconfigure the daemon; `up` may wait for a login.
pub const ACTION_TIMEOUT: Duration = Duration::from_secs(30);
/// How often a running child is checked while its output is quiet.
const POLL: Duration = Duration::from_millis(50);
/// After the child exits, its pipes get this long to drain.
const DRAIN: Duration = Duration::from_millis(500);
/// Timestamps before this year are Go's zero time ("never").
const FIRST_REAL_YEAR: i32 = 2000;
/// What to run once when the CLI refuses because the user isn't the operator.
pub const OPERATOR_HINT: &str = "sudo tailscale set --operator=$USER";

/// The daemon's state (`BackendState`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Backend {
    Running,
    Stopped,
    NeedsLogin,
    NeedsMachineAuth,
    Starting,
    NoState,
    Other(String),
}

impl Backend {
    pub fn parse(s: &str) -> Self {
        match s {
            "Running" => Self::Running,
            "Stopped" => Self::Stopped,
            "NeedsLogin" => Self::NeedsLogin,
            "NeedsMachineAuth" => Self::NeedsMachineAuth,
            "Starting" => Self::Starting,
            "NoState" | "" => Self::NoState,
            other => Self::Other(other.to_string()),
        }
    }

    /// Words for the UI.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Running => t("Connected"),
            Self::Stopped => t("Disconnected"),
            Self::NeedsLogin => t("Logged out"),
            Self::NeedsMachineAuth => t("Waiting for admin approval"),
            Self::Starting => t("Connecting…"),
            Self::NoState | Self::Other(_) => t("Not running"),
        }
    }
}

/// This device or a peer.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Node {
    /// Node key (stable id).
    pub id: String,
    /// The name shown: the host name, else the first MagicDNS label.
    pub name: String,
    /// MagicDNS name without the trailing dot.
    pub dns_name: String,
    pub ips: Vec<String>,
    pub os: String,
    pub online: bool,
    /// This peer is the exit node in use.
    pub exit_node: bool,
    /// This peer offers itself as an exit node.
    pub exit_node_option: bool,
    /// Unix seconds; `None` when never seen (or online now).
    pub last_seen: Option<i64>,
    /// DERP region relaying traffic when there is no direct path.
    pub relay: String,
    /// The direct `ip:port` in use (empty when relayed or idle).
    pub cur_addr: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

impl Node {
    /// The first Tailscale address (IPv4 when there is one).
    pub fn ip(&self) -> Option<&str> {
        self.ips.iter().find(|ip| !ip.contains(':')).or(self.ips.first()).map(String::as_str)
    }

    /// Traffic flows peer to peer (not through a DERP relay).
    pub fn direct(&self) -> bool {
        !self.cur_addr.is_empty()
    }

    /// Whether `query` names this node: its name, MagicDNS name or an IP.
    pub fn matches(&self, query: &str) -> bool {
        let q = query.trim().trim_end_matches('.');
        !q.is_empty()
            && (self.name.eq_ignore_ascii_case(q)
                || self.dns_name.eq_ignore_ascii_case(q)
                || self.dns_name.split('.').next().is_some_and(|l| l.eq_ignore_ascii_case(q))
                || self.ips.iter().any(|ip| ip == q))
    }
}

/// One `tailscale status --json`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Status {
    pub backend: Backend,
    pub version: String,
    /// The tailnet's name (`CurrentTailnet.Name`).
    pub tailnet: Option<String>,
    /// This device (absent before the first login).
    pub this: Option<Node>,
    /// Online peers first, then by name.
    pub peers: Vec<Node>,
    /// A login link the daemon is waiting on.
    pub auth_url: Option<String>,
    /// Health warnings the daemon reports.
    pub health: Vec<String>,
}

impl Status {
    pub fn connected(&self) -> bool {
        self.backend == Backend::Running
    }

    /// The exit node in use.
    pub fn exit_node(&self) -> Option<&Node> {
        self.peers.iter().find(|p| p.exit_node)
    }

    /// Peers offering themselves as exit nodes.
    pub fn exit_options(&self) -> impl Iterator<Item = &Node> {
        self.peers.iter().filter(|p| p.exit_node_option)
    }

    pub fn online_count(&self) -> usize {
        self.peers.iter().filter(|p| p.online).count()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawStatus {
    #[serde(default)]
    version: String,
    #[serde(default)]
    backend_state: String,
    #[serde(default, rename = "AuthURL")]
    auth_url: String,
    #[serde(default, rename = "Self")]
    this: Option<RawNode>,
    #[serde(default)]
    peer: Option<std::collections::BTreeMap<String, RawNode>>,
    #[serde(default)]
    current_tailnet: Option<RawTailnet>,
    #[serde(default)]
    health: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawTailnet {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawNode {
    #[serde(default, rename = "PublicKey")]
    public_key: String,
    #[serde(default)]
    host_name: String,
    #[serde(default, rename = "DNSName")]
    dns_name: String,
    #[serde(default, rename = "OS")]
    os: String,
    #[serde(default, rename = "TailscaleIPs")]
    tailscale_ips: Option<Vec<String>>,
    #[serde(default)]
    online: bool,
    #[serde(default)]
    exit_node: bool,
    #[serde(default)]
    exit_node_option: bool,
    #[serde(default)]
    last_seen: String,
    #[serde(default)]
    relay: String,
    #[serde(default)]
    cur_addr: String,
    #[serde(default)]
    rx_bytes: u64,
    #[serde(default)]
    tx_bytes: u64,
}

/// Unix seconds of an RFC 3339 time; Go's zero time is "never".
fn seen_at(s: &str) -> Option<i64> {
    use chrono::Datelike as _;
    let at = chrono::DateTime::parse_from_rfc3339(s).ok()?;
    (at.year() >= FIRST_REAL_YEAR).then(|| at.timestamp())
}

fn node(key: &str, raw: RawNode) -> Node {
    let dns_name = raw.dns_name.trim_end_matches('.').to_string();
    let first_label = dns_name.split('.').next().unwrap_or_default().to_string();
    // Phones often report `localhost`; the MagicDNS label is what people know.
    let name = match raw.host_name.trim() {
        "" | "localhost" if !first_label.is_empty() => first_label,
        h => h.to_string(),
    };
    Node {
        id: if raw.public_key.is_empty() { key.to_string() } else { raw.public_key },
        name,
        dns_name,
        ips: raw.tailscale_ips.unwrap_or_default(),
        os: raw.os,
        online: raw.online,
        exit_node: raw.exit_node,
        exit_node_option: raw.exit_node_option,
        last_seen: if raw.online { None } else { seen_at(&raw.last_seen) },
        relay: raw.relay,
        cur_addr: raw.cur_addr,
        rx_bytes: raw.rx_bytes,
        tx_bytes: raw.tx_bytes,
    }
}

/// Parses `tailscale status --json`.
pub fn parse_status(json: &str) -> Result<Status, String> {
    let raw: RawStatus = serde_json::from_str(json).map_err(|e| tf("Unexpected answer from tailscale: {error}", &[("error", &e)]))?;
    let mut peers: Vec<Node> = raw.peer.unwrap_or_default().into_iter().map(|(k, n)| node(&k, n)).collect();
    peers.sort_by(|a, b| b.online.cmp(&a.online).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(Status {
        backend: Backend::parse(&raw.backend_state),
        version: raw.version.split('-').next().unwrap_or_default().to_string(),
        tailnet: raw.current_tailnet.map(|t| t.name).filter(|n| !n.is_empty()),
        this: raw.this.map(|n| node("self", n)),
        peers,
        auth_url: Some(raw.auth_url).filter(|u| !u.is_empty()),
        health: raw.health.unwrap_or_default(),
    })
}

/// Why a CLI call failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// No `tailscale` on `PATH`.
    NotInstalled,
    /// `tailscaled` isn't running (the CLI's own message).
    Daemon(String),
    /// The CLI refused because this user may not change settings.
    NotOperator(String),
    /// Anything else, as the CLI put it.
    Failed(String),
}

impl Error {
    pub fn message(&self) -> String {
        match self {
            Self::NotInstalled => t("Tailscale isn't installed").into(),
            Self::Daemon(m) | Self::NotOperator(m) | Self::Failed(m) => m.clone(),
        }
    }

    /// Classifies a failed run by what the CLI printed.
    fn from_output(text: &str) -> Self {
        let msg = text.trim().to_string();
        let lower = msg.to_lowercase();
        if needs_operator(&lower) {
            Self::NotOperator(msg)
        } else if lower.contains("tailscaled") && (lower.contains("not running") || lower.contains("doesn't appear to be running") || lower.contains("failed to connect")) {
            Self::Daemon(msg)
        } else if msg.is_empty() {
            Self::Failed(t("tailscale failed without a message").into())
        } else {
            Self::Failed(msg)
        }
    }
}

/// Whether the CLI's message says the user must be the operator (or root).
pub fn needs_operator(message: &str) -> bool {
    let m = message.to_lowercase();
    m.contains("access denied") || m.contains("--operator") || m.contains("permission denied")
}

/// What a finished (or still waiting) CLI run printed.
struct Run {
    stdout: String,
    stderr: String,
    /// `None`: still running (left waiting on a login link).
    success: Option<bool>,
    auth_url: Option<String>,
}

/// The first `https://` link in a line of CLI output.
pub fn find_url(line: &str) -> Option<String> {
    let start = line.find("https://")?;
    let url: String = line[start..].chars().take_while(|c| !c.is_whitespace()).collect();
    (url.len() > "https://".len()).then_some(url)
}

fn pipe_lines(pipe: Option<impl Read + Send + 'static>, stderr: bool, tx: &mpsc::Sender<(bool, String)>) {
    let Some(pipe) = pipe else { return };
    let tx = tx.clone();
    let _ = std::thread::Builder::new().name("mandara-tailscale-pipe".into()).spawn(move || {
        for line in BufReader::new(pipe).lines().map_while(Result::ok) {
            if tx.send((stderr, line)).is_err() {
                break;
            }
        }
    });
}

/// Runs `tailscale args…` until it exits or `timeout` passes. With
/// `until_url`, returns as soon as a login link is printed and leaves the
/// CLI running in the background.
fn run(args: &[&str], timeout: Duration, until_url: bool) -> Result<Run, Error> {
    let mut child: Child = Command::new(BIN)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { Error::NotInstalled } else { Error::Failed(e.to_string()) })?;
    let (tx, rx) = mpsc::channel();
    pipe_lines(child.stdout.take(), false, &tx);
    pipe_lines(child.stderr.take(), true, &tx);
    drop(tx);
    let deadline = Instant::now() + timeout;
    let (mut stdout, mut stderr) = (String::new(), String::new());
    let mut exited: Option<bool> = None;
    let mut drain_until: Option<Instant> = None;
    loop {
        match rx.recv_timeout(POLL) {
            Ok((is_err, line)) => {
                let url = if until_url { find_url(&line) } else { None };
                let buf = if is_err { &mut stderr } else { &mut stdout };
                buf.push_str(&line);
                buf.push('\n');
                if let Some(url) = url
                    && exited.is_none()
                {
                    crate::reaper::adopt(child);
                    return Ok(Run { stdout, stderr, success: None, auth_url: Some(url) });
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let ok = match exited {
                    Some(ok) => ok,
                    None => child.wait().map(|s| s.success()).unwrap_or(false),
                };
                return Ok(Run { stdout, stderr, success: Some(ok), auth_url: None });
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if exited.is_none()
            && let Ok(Some(status)) = child.try_wait()
        {
            exited = Some(status.success());
            drain_until = Some(Instant::now() + DRAIN);
        }
        if drain_until.is_some_and(|d| Instant::now() > d) {
            return Ok(Run { stdout, stderr, success: exited, auth_url: None });
        }
        if exited.is_none() && Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Failed(tf("tailscale {command} didn't finish within {seconds} s", &[("command", &args.join(" ")), ("seconds", &timeout.as_secs())])));
        }
    }
}

fn checked(args: &[&str], timeout: Duration) -> Result<Run, Error> {
    let r = run(args, timeout, false)?;
    if r.success == Some(true) {
        Ok(r)
    } else {
        Err(Error::from_output(if r.stderr.trim().is_empty() { &r.stdout } else { &r.stderr }))
    }
}

/// `tailscale status --json`.
pub fn status() -> Result<Status, Error> {
    let r = run(&["status", "--json"], STATUS_TIMEOUT, false)?;
    // Logged out or stopped still prints JSON (and may exit non-zero).
    match parse_status(&r.stdout) {
        Ok(s) => Ok(s),
        Err(e) if r.success == Some(true) => Err(Error::Failed(e)),
        Err(_) => Err(Error::from_output(if r.stderr.trim().is_empty() { &r.stdout } else { &r.stderr })),
    }
}

/// `tailscale up`. `Ok(Some(url))` when the device must log in first: open
/// the link to finish.
///
/// A bare `tailscale up` refuses when settings were changed before (an
/// exit node chosen with `tailscale set`, say) and prints the command that
/// keeps them; that command is run instead, so Connect never resets them.
pub fn up() -> Result<Option<String>, Error> {
    let r = run(&["up"], ACTION_TIMEOUT, true)?;
    let r = match (r.success, &r.auth_url) {
        (Some(false), None) => match keep_settings_args(&r.stderr) {
            Some(flags) => {
                let args: Vec<&str> = std::iter::once("up").chain(flags.iter().map(String::as_str)).collect();
                run(&args, ACTION_TIMEOUT, true)?
            }
            None => r,
        },
        _ => r,
    };
    match (r.success, r.auth_url) {
        (_, Some(url)) => Ok(Some(url)),
        (Some(true), None) => Ok(None),
        _ => Err(Error::from_output(if r.stderr.trim().is_empty() { &r.stdout } else { &r.stderr })),
    }
}

/// The flags of the `tailscale up --flag=value …` line the CLI suggests
/// when a bare `up` would change non-default settings (flags only).
fn keep_settings_args(stderr: &str) -> Option<Vec<String>> {
    if !stderr.contains("non-default") {
        return None;
    }
    let line = stderr.lines().map(str::trim).find(|l| l.starts_with("tailscale up "))?;
    let flags: Vec<String> = line.split_whitespace().skip(2).map(String::from).collect();
    (!flags.is_empty() && flags.iter().all(|f| f.starts_with("--") && f != "--reset")).then_some(flags)
}

/// `tailscale down`.
pub fn down() -> Result<(), Error> {
    checked(&["down"], ACTION_TIMEOUT).map(drop)
}

/// `tailscale set --exit-node=<ip>`, or no exit node with `None`.
pub fn set_exit_node(ip: Option<&str>) -> Result<(), Error> {
    let arg = format!("--exit-node={}", ip.unwrap_or_default());
    checked(&["set", &arg], ACTION_TIMEOUT).map(drop)
}

/// The exit-node-capable peer `query` names (name, MagicDNS name or IP).
pub fn resolve_exit_node<'a>(status: &'a Status, query: &str) -> Result<&'a Node, String> {
    status.exit_options().find(|p| p.matches(query)).ok_or_else(|| tf("No exit node matches {query}", &[("query", &query)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
  "Version": "1.90.6-t1234abcd-g5678",
  "TUN": true,
  "BackendState": "Running",
  "HaveNodeKey": true,
  "AuthURL": "",
  "TailscaleIPs": ["100.101.102.103", "fd7a:115c:a1e0::1"],
  "Self": {
    "ID": "nSelf", "PublicKey": "nodekey:self", "HostName": "corniche",
    "DNSName": "corniche.tail1234.ts.net.", "OS": "linux", "UserID": 1,
    "TailscaleIPs": ["100.101.102.103", "fd7a:115c:a1e0::1"],
    "Relay": "fra", "CurAddr": "", "RxBytes": 0, "TxBytes": 0,
    "Created": "2025-01-01T00:00:00Z", "LastSeen": "0001-01-01T00:00:00Z",
    "Online": true, "ExitNode": false, "ExitNodeOption": false, "Active": false
  },
  "Health": ["Some peers are advertising routes but --accept-routes is false"],
  "MagicDNSSuffix": "tail1234.ts.net",
  "CurrentTailnet": { "Name": "hamdi@example.com", "MagicDNSSuffix": "tail1234.ts.net", "MagicDNSEnabled": true },
  "Peer": {
    "nodekey:aaa": {
      "ID": "n1", "PublicKey": "nodekey:aaa", "HostName": "nas", "DNSName": "nas.tail1234.ts.net.", "OS": "linux",
      "TailscaleIPs": ["100.64.0.2", "fd7a:115c:a1e0::2"], "Relay": "fra", "CurAddr": "192.168.1.20:41641",
      "RxBytes": 1048576, "TxBytes": 2048, "LastSeen": "2026-10-30T12:00:00Z",
      "Online": true, "ExitNode": true, "ExitNodeOption": true, "Active": true
    },
    "nodekey:bbb": {
      "ID": "n2", "PublicKey": "nodekey:bbb", "HostName": "localhost", "DNSName": "pixel-9.tail1234.ts.net.", "OS": "android",
      "TailscaleIPs": ["100.64.0.3"], "Relay": "ams", "CurAddr": "",
      "RxBytes": 0, "TxBytes": 0, "LastSeen": "2026-10-29T08:30:00Z",
      "Online": false, "ExitNode": false, "ExitNodeOption": false, "Active": false
    },
    "nodekey:ccc": {
      "ID": "n3", "PublicKey": "nodekey:ccc", "HostName": "Beach-Mac", "DNSName": "beach-mac.tail1234.ts.net.", "OS": "macOS",
      "TailscaleIPs": ["100.64.0.4"], "Relay": "fra", "CurAddr": "",
      "LastSeen": "2026-10-30T12:00:00Z", "Online": true, "ExitNode": false, "ExitNodeOption": true
    }
  }
}"#;

    #[test]
    fn parses_a_running_tailnet() {
        let s = parse_status(SAMPLE).unwrap();
        assert_eq!(s.backend, Backend::Running);
        assert!(s.connected());
        assert_eq!(s.version, "1.90.6");
        assert_eq!(s.tailnet.as_deref(), Some("hamdi@example.com"));
        let me = s.this.as_ref().unwrap();
        assert_eq!(me.name, "corniche");
        assert_eq!(me.dns_name, "corniche.tail1234.ts.net");
        assert_eq!(me.ip(), Some("100.101.102.103"));
        assert_eq!(me.last_seen, None);
        assert_eq!(s.health.len(), 1);
    }

    #[test]
    fn peers_sort_online_first_and_keep_their_paths() {
        let s = parse_status(SAMPLE).unwrap();
        let names: Vec<&str> = s.peers.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Beach-Mac", "nas", "pixel-9"], "online first, then by name; a phone's localhost becomes its MagicDNS label");
        let nas = &s.peers[1];
        assert!(nas.direct() && nas.exit_node && nas.exit_node_option);
        assert_eq!(nas.rx_bytes, 1_048_576);
        assert_eq!(nas.last_seen, None, "online peers have no last-seen");
        let mac = &s.peers[0];
        assert!(!mac.direct());
        assert_eq!(mac.relay, "fra");
        let phone = &s.peers[2];
        assert!(!phone.online);
        assert_eq!(phone.last_seen, Some(chrono::DateTime::parse_from_rfc3339("2026-10-29T08:30:00Z").unwrap().timestamp()));
        assert_eq!(s.exit_node().map(|n| n.name.as_str()), Some("nas"));
        assert_eq!(s.exit_options().count(), 2);
        assert_eq!(s.online_count(), 2);
    }

    #[test]
    fn exit_nodes_resolve_by_name_dns_or_ip() {
        let s = parse_status(SAMPLE).unwrap();
        assert_eq!(resolve_exit_node(&s, "NAS").unwrap().name, "nas");
        assert_eq!(resolve_exit_node(&s, "beach-mac.tail1234.ts.net.").unwrap().name, "Beach-Mac");
        assert_eq!(resolve_exit_node(&s, "100.64.0.2").unwrap().name, "nas");
        assert!(resolve_exit_node(&s, "pixel-9").is_err(), "not an exit node");
    }

    #[test]
    fn logged_out_devices_parse_without_peers() {
        let s = parse_status(r#"{"Version":"1.90.6","BackendState":"NeedsLogin","AuthURL":"https://login.tailscale.com/a/abc123","Self":null,"Peer":null,"Health":null}"#).unwrap();
        assert_eq!(s.backend, Backend::NeedsLogin);
        assert!(s.this.is_none() && s.peers.is_empty());
        assert_eq!(s.auth_url.as_deref(), Some("https://login.tailscale.com/a/abc123"));
        assert!(parse_status("not json").is_err());
    }

    #[test]
    fn cli_errors_are_classified() {
        assert!(matches!(Error::from_output("Access denied: prefs write access denied\n\nUse 'sudo tailscale up' or 'tailscale set --operator=$USER'"), Error::NotOperator(_)));
        assert!(matches!(Error::from_output("failed to connect to local tailscaled; it doesn't appear to be running (sudo systemctl start tailscaled ?)"), Error::Daemon(_)));
        assert!(matches!(Error::from_output("boom"), Error::Failed(m) if m == "boom"));
        assert_eq!(find_url("To authenticate, visit:\thttps://login.tailscale.com/a/1f2e3d "), Some("https://login.tailscale.com/a/1f2e3d".into()));
        assert_eq!(find_url("no link here"), None);
    }

    #[test]
    fn up_keeps_earlier_settings() {
        let refusal = "Error: changing settings via 'tailscale up' requires mentioning all\nnon-default flags. To proceed, either re-run your command with --reset or\nuse the command below to explicitly mention the current value of\nall non-default settings:\n\n\ttailscale up --exit-node=100.64.0.2 --accept-routes\n";
        assert_eq!(keep_settings_args(refusal), Some(vec!["--exit-node=100.64.0.2".to_string(), "--accept-routes".to_string()]));
        assert_eq!(keep_settings_args("some other failure"), None);
        assert_eq!(keep_settings_args("non-default flags\n\ttailscale up --reset"), None, "never resets");
    }
}
