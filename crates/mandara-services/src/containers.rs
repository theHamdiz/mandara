//! Docker and Podman through their HTTP APIs on local Unix sockets.
//!
//! There is no HTTP library in the tree, so this speaks a minimal subset of
//! HTTP/1.1 over a `UnixStream`: one request per connection
//! (`Connection: close`), `Content-Length` or `chunked` bodies, and a read
//! timeout so a wedged daemon can't stall a thread. Requests go through the
//! [`Transport`] trait, which lets the tests check what each mutation sends
//! with a recording mock. They never touch a real engine.
//!
//! Both engines serve the Docker-compatible API on unversioned paths
//! (`/containers/json`, …), so one parser covers them. Podman returns
//! `null` where Docker returns `[]`/`{}`, and names with or without the
//! leading `/`, so parsing is lenient.
//!
//! [`spawn_monitor`] runs a poller thread shared by the bar chip and the
//! panel. It lists containers at a slow pace while the chip exists. While
//! the panel is open ([`Monitor::ping_detail`]) it also samples stats,
//! inspects start times and counts images and volumes. The thread exits
//! once every [`Monitor`] handle is dropped.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use mandara_i18n::{t, tf};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::Value;

/// How long one request may take (connect, write, read).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(6);
/// Responses larger than this are refused (log tails are bounded anyway).
pub const MAX_RESPONSE: usize = 32 * 1024 * 1024;
/// Container list cadence while only the bar chip is visible.
pub const LIST_EVERY: Duration = Duration::from_secs(30);
/// Bounds of the list cadence setting (`refresh_s`).
pub const LIST_EVERY_RANGE: (Duration, Duration) = (Duration::from_secs(5), Duration::from_secs(600));
/// List + stats cadence while the panel is open.
pub const DETAIL_EVERY: Duration = Duration::from_secs(2);
/// Image / volume counts cadence while the panel is open.
pub const COUNTS_EVERY: Duration = Duration::from_secs(30);
/// The panel counts as open for this long after its last ping.
pub const DETAIL_TTL: Duration = Duration::from_secs(4);
/// Default number of log lines a logs view loads.
pub const DEFAULT_LOG_LINES: usize = 200;
/// Hard ceiling for requested log lines.
pub const MAX_LOG_LINES: usize = 5000;
/// Docker's "never" timestamp prefix (`0001-01-01T00:00:00Z`).
const ZERO_TIME_PREFIX: &str = "0001-";
/// Compose project / service labels (Docker Compose, podman-compose).
const PROJECT_LABELS: [&str; 2] = ["com.docker.compose.project", "io.podman.compose.project"];
const SERVICE_LABELS: [&str; 2] = ["com.docker.compose.service", "io.podman.compose.service"];
/// Bytes in a multiplexed log frame header.
const FRAME_HEADER: usize = 8;

// ── Engines and endpoints ────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Docker,
    Podman,
}

impl Engine {
    pub fn label(self) -> &'static str {
        match self {
            Engine::Docker => "Docker",
            Engine::Podman => "Podman",
        }
    }

    pub fn parse(s: &str) -> Option<Engine> {
        match s.trim().to_ascii_lowercase().as_str() {
            "docker" => Some(Engine::Docker),
            "podman" => Some(Engine::Podman),
            _ => None,
        }
    }
}

/// Which engine to use: the `engine` setting (`auto`, `docker`, `podman`).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Preference {
    #[default]
    Auto,
    Only(Engine),
}

impl Preference {
    pub fn parse(s: &str) -> Preference {
        Engine::parse(s).map(Preference::Only).unwrap_or(Preference::Auto)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Endpoint {
    pub engine: Engine,
    pub socket: PathBuf,
}

/// Candidate sockets in priority order (they may not exist).
pub fn candidates(docker_host: Option<&str>, runtime_dir: Option<&Path>, home: Option<&Path>) -> Vec<Endpoint> {
    let mut out = Vec::new();
    let mut push = |engine: Engine, socket: PathBuf| {
        if !out.iter().any(|e: &Endpoint| e.socket == socket) {
            out.push(Endpoint { engine, socket });
        }
    };
    if let Some(path) = docker_host.and_then(|h| h.strip_prefix("unix://")) {
        // DOCKER_HOST may point at Podman's Docker-compatible socket.
        let engine = if path.contains("podman") { Engine::Podman } else { Engine::Docker };
        push(engine, PathBuf::from(path));
    }
    push(Engine::Docker, PathBuf::from("/var/run/docker.sock"));
    if let Some(home) = home {
        push(Engine::Docker, home.join(".docker/desktop/docker.sock"));
    }
    if let Some(run) = runtime_dir {
        push(Engine::Podman, run.join("podman/podman.sock"));
    }
    push(Engine::Podman, PathBuf::from("/run/podman/podman.sock"));
    out
}

/// Engines whose socket exists on this machine (one per engine, first wins).
pub fn discover() -> Vec<Endpoint> {
    let host = std::env::var("DOCKER_HOST").ok();
    let run = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let mut seen: HashSet<Engine> = HashSet::new();
    let mut canon: HashSet<PathBuf> = HashSet::new();
    candidates(host.as_deref(), run.as_deref(), dirs::home_dir().as_deref())
        .into_iter()
        .filter(|e| is_socket(&e.socket))
        // /var/run is usually a symlink to /run: skip duplicates.
        .filter(|e| canon.insert(std::fs::canonicalize(&e.socket).unwrap_or_else(|_| e.socket.clone())))
        .filter(|e| seen.insert(e.engine))
        .collect()
}

fn is_socket(p: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt as _;
    std::fs::metadata(p).map(|m| m.file_type().is_socket()).unwrap_or(false)
}

/// The endpoint to use for `pref`, given the discovered ones.
pub fn choose(endpoints: &[Endpoint], pref: Preference) -> Option<Endpoint> {
    match pref {
        Preference::Auto => endpoints.first().cloned(),
        Preference::Only(e) => endpoints.iter().find(|x| x.engine == e).cloned(),
    }
}

// ── Errors ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ApiError {
    /// No Docker / Podman socket exists.
    NoEngine,
    /// The socket exists but this user may not open it.
    PermissionDenied { socket: String },
    /// Connecting, reading or writing failed.
    Io { message: String },
    /// The engine answered with an error status.
    Http { status: u16, message: String },
    /// A response we could not understand.
    Protocol { message: String },
    NotFound { name: String },
    InvalidName { name: String },
}

impl ApiError {
    /// One-line explanation in the interface language.
    pub fn message(&self) -> String {
        match self {
            ApiError::NoEngine => t("No Docker or Podman socket found").into(),
            ApiError::PermissionDenied { socket } => tf("Permission denied on {socket}", &[("socket", socket)]),
            ApiError::Io { message } => tf("Could not reach the engine: {error}", &[("error", message)]),
            ApiError::Http { status, message } => tf("The engine refused ({status}): {error}", &[("status", status), ("error", message)]),
            ApiError::Protocol { message } => tf("Unexpected reply from the engine: {error}", &[("error", message)]),
            ApiError::NotFound { name } => tf("No container named \u{201c}{name}\u{201d}", &[("name", name)]),
            ApiError::InvalidName { name } => tf("\u{201c}{name}\u{201d} is not a valid container name", &[("name", name)]),
        }
    }

    /// What the user can do about it (empty when there is nothing to add).
    pub fn hint(&self) -> String {
        match self {
            ApiError::NoEngine => t("Start the Docker or Podman service (for Podman: systemctl --user enable --now podman.socket)").into(),
            ApiError::PermissionDenied { .. } => t("Join the docker group (sudo usermod -aG docker $USER), then log out and back in").into(),
            ApiError::Io { .. } => t("Is the engine running? Mandara retries automatically").into(),
            _ => String::new(),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

// ── HTTP/1.1 over a Unix socket ──────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The engine's `{"message": "…"}` error text (or the raw body).
    pub fn error_message(&self) -> String {
        serde_json::from_slice::<Value>(&self.body)
            .ok()
            .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).trim().to_string())
    }
}

/// Sends one HTTP request and returns the response.
pub trait Transport {
    fn request(&mut self, method: &str, path: &str) -> Result<Response, ApiError>;
}

/// The real transport: a fresh `UnixStream` per request.
pub struct UnixTransport {
    pub socket: PathBuf,
    pub timeout: Duration,
}

impl UnixTransport {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self { socket: socket.into(), timeout: REQUEST_TIMEOUT }
    }
}

/// The request bytes for `method path` (no body; POSTs send `Content-Length: 0`).
pub fn request_bytes(method: &str, path: &str) -> Vec<u8> {
    let mut s = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nUser-Agent: mandara-shell\r\nAccept: application/json\r\nConnection: close\r\n");
    if method != "GET" {
        s.push_str("Content-Length: 0\r\n");
    }
    s.push_str("\r\n");
    s.into_bytes()
}

impl Transport for UnixTransport {
    fn request(&mut self, method: &str, path: &str) -> Result<Response, ApiError> {
        let mut stream = UnixStream::connect(&self.socket).map_err(|e| match e.kind() {
            std::io::ErrorKind::PermissionDenied => ApiError::PermissionDenied { socket: self.socket.display().to_string() },
            std::io::ErrorKind::NotFound => ApiError::NoEngine,
            _ => ApiError::Io { message: e.to_string() },
        })?;
        let io = |e: std::io::Error| ApiError::Io { message: e.to_string() };
        stream.set_read_timeout(Some(self.timeout)).map_err(io)?;
        stream.set_write_timeout(Some(self.timeout)).map_err(io)?;
        stream.write_all(&request_bytes(method, path)).map_err(io)?;
        let deadline = Instant::now() + self.timeout;
        let mut raw = Vec::new();
        let mut buf = [0u8; 16 * 1024];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    raw.extend_from_slice(&buf[..n]);
                    if raw.len() > MAX_RESPONSE {
                        return Err(ApiError::Protocol { message: t("response too large").into() });
                    }
                    if is_complete(&raw) {
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(io(e)),
            }
            if Instant::now() > deadline {
                return Err(ApiError::Io { message: t("timed out").into() });
            }
        }
        parse_response(&raw)
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Whether `raw` already holds a full response (so we needn't wait for EOF).
fn is_complete(raw: &[u8]) -> bool {
    let Some(end) = find(raw, b"\r\n\r\n") else { return false };
    let Ok(head) = std::str::from_utf8(&raw[..end]) else { return false };
    let body = &raw[end + 4..];
    let header = |name: &str| head.lines().skip(1).find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_string()));
    if header("transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) {
        return dechunk(body).is_ok();
    }
    match header("content-length").and_then(|v| v.parse::<usize>().ok()) {
        Some(len) => body.len() >= len,
        // 1xx / 204 / 304 carry no body.
        None => head.split_whitespace().nth(1).is_some_and(|s| s == "204" || s == "304"),
    }
}

/// Decodes a `chunked` body; errors if it is truncated.
pub fn dechunk(mut body: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let line_end = find(body, b"\r\n").ok_or("truncated chunk size")?;
        let size_text = std::str::from_utf8(&body[..line_end]).map_err(|e| e.to_string())?;
        let size_hex = size_text.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_hex, 16).map_err(|_| format!("bad chunk size `{size_hex}`"))?;
        body = &body[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if body.len() < size + 2 {
            return Err("truncated chunk".into());
        }
        out.extend_from_slice(&body[..size]);
        body = &body[size + 2..];
    }
}

/// Parses a raw HTTP/1.1 response.
pub fn parse_response(raw: &[u8]) -> Result<Response, ApiError> {
    let proto = |m: &str| ApiError::Protocol { message: m.to_string() };
    let end = find(raw, b"\r\n\r\n").ok_or_else(|| proto("no header terminator"))?;
    let head = std::str::from_utf8(&raw[..end]).map_err(|_| proto("headers are not UTF-8"))?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = status_line.split_whitespace().nth(1).and_then(|s| s.parse::<u16>().ok()).ok_or_else(|| proto("bad status line"))?;
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))).collect();
    let mut resp = Response { status, headers, body: Vec::new() };
    let body = &raw[end + 4..];
    resp.body = if resp.header("transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) {
        dechunk(body).map_err(|e| proto(&e))?
    } else if let Some(len) = resp.header("content-length").and_then(|v| v.parse::<usize>().ok()) {
        body.get(..len).ok_or_else(|| proto("truncated body"))?.to_vec()
    } else {
        body.to_vec()
    };
    Ok(resp)
}

// ── Model ────────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Running,
    Paused,
    Restarting,
    Created,
    Exited,
    Removing,
    Dead,
    Unknown,
}

impl State {
    pub fn parse(s: &str) -> State {
        match s.to_ascii_lowercase().as_str() {
            "running" => State::Running,
            "paused" => State::Paused,
            "restarting" => State::Restarting,
            "created" | "configured" | "initialized" => State::Created,
            "exited" | "stopped" => State::Exited,
            "removing" => State::Removing,
            "dead" => State::Dead,
            _ => State::Unknown,
        }
    }

    /// Up (running, paused or restarting): counts as live, stats apply.
    pub fn is_up(self) -> bool {
        matches!(self, State::Running | State::Paused | State::Restarting)
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Health {
    Healthy,
    Unhealthy,
    Starting,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Port {
    pub private: u16,
    pub public: Option<u16>,
    pub ip: String,
    pub proto: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: State,
    /// The engine's own status text (`Up 3 hours (healthy)`), English.
    pub status: String,
    pub health: Option<Health>,
    /// Exit code parsed from the status of an exited container.
    pub exit_code: Option<i64>,
    /// Unix seconds.
    pub created: i64,
    pub ports: Vec<Port>,
    pub project: Option<String>,
    pub service: Option<String>,
}

impl Container {
    pub fn short_id(&self) -> &str {
        &self.id[..self.id.len().min(12)]
    }

    /// Ports deduplicated across IPv4 / IPv6 bindings.
    pub fn unique_ports(&self) -> Vec<Port> {
        let mut out: Vec<Port> = Vec::new();
        for p in &self.ports {
            if !out.iter().any(|q| q.private == p.private && q.public == p.public && q.proto == p.proto) {
                out.push(p.clone());
            }
        }
        out.sort_by_key(|p| (p.public.is_none(), p.public, p.private));
        out
    }

    /// Whether `query` matches the name, image, project or ID.
    pub fn matches(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        q.is_empty()
            || self.name.to_lowercase().contains(&q)
            || self.image.to_lowercase().contains(&q)
            || self.project.as_deref().is_some_and(|p| p.to_lowercase().contains(&q))
            || self.id.starts_with(&q)
    }
}

fn label(labels: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| labels.get(*k).and_then(Value::as_str)).map(str::to_string).filter(|s| !s.is_empty())
}

fn health_of(status: &str) -> Option<Health> {
    let s = status.to_ascii_lowercase();
    if s.contains("(unhealthy)") {
        Some(Health::Unhealthy)
    } else if s.contains("(healthy)") {
        Some(Health::Healthy)
    } else if s.contains("health: starting") {
        Some(Health::Starting)
    } else {
        None
    }
}

/// `Exited (137) 3 hours ago` → `137`.
fn exit_code_of(status: &str) -> Option<i64> {
    let rest = status.trim().strip_prefix("Exited")?.trim_start().strip_prefix('(')?;
    rest.split(')').next()?.trim().parse().ok()
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// Parses `GET /containers/json` (Docker or Podman compat).
pub fn parse_containers(body: &[u8]) -> Result<Vec<Container>, ApiError> {
    let v: Value = serde_json::from_slice(body).map_err(|e| ApiError::Protocol { message: e.to_string() })?;
    let arr = v.as_array().ok_or_else(|| ApiError::Protocol { message: "expected an array".into() })?;
    let mut out: Vec<Container> = arr
        .iter()
        .map(|c| {
            let names: Vec<String> = c.get("Names").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(|s| s.trim_start_matches('/').to_string()).collect()).unwrap_or_default();
            let id = str_of(c, "Id");
            let name = names.into_iter().next().unwrap_or_else(|| id.chars().take(12).collect());
            let status = str_of(c, "Status");
            let labels = c.get("Labels").cloned().unwrap_or(Value::Null);
            let ports = c
                .get("Ports")
                .and_then(Value::as_array)
                .map(|ps| {
                    ps.iter()
                        .filter_map(|p| {
                            // Docker: PrivatePort/PublicPort/Type; Podman libpod-ish: container_port/host_port/protocol.
                            let private = p.get("PrivatePort").or_else(|| p.get("container_port")).and_then(Value::as_u64)?;
                            let public = p.get("PublicPort").or_else(|| p.get("host_port")).and_then(Value::as_u64).filter(|n| *n > 0);
                            let ip = p.get("IP").or_else(|| p.get("host_ip")).and_then(Value::as_str).unwrap_or_default().to_string();
                            let proto = p.get("Type").or_else(|| p.get("protocol")).and_then(Value::as_str).unwrap_or("tcp").to_string();
                            Some(Port { private: private as u16, public: public.map(|n| n as u16), ip, proto })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let state = State::parse(&str_of(c, "State"));
            Container {
                image: str_of(c, "Image"),
                health: health_of(&status),
                exit_code: if state == State::Exited { exit_code_of(&status) } else { None },
                created: c.get("Created").and_then(Value::as_i64).unwrap_or(0),
                project: label(&labels, &PROJECT_LABELS),
                service: label(&labels, &SERVICE_LABELS),
                state,
                status,
                ports,
                id,
                name,
            }
        })
        .collect();
    out.sort_by(|a, b| b.state.is_up().cmp(&a.state.is_up()).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(out)
}

/// What `GET /containers/{id}/json` tells us beyond the list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Inspect {
    /// Unix seconds; `None` if never started.
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub exit_code: Option<i64>,
    pub tty: bool,
}

fn rfc3339(s: &str) -> Option<i64> {
    if s.is_empty() || s.starts_with(ZERO_TIME_PREFIX) {
        return None;
    }
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp())
}

pub fn parse_inspect(body: &[u8]) -> Result<Inspect, ApiError> {
    let v: Value = serde_json::from_slice(body).map_err(|e| ApiError::Protocol { message: e.to_string() })?;
    let st = v.get("State").cloned().unwrap_or(Value::Null);
    Ok(Inspect {
        started_at: st.get("StartedAt").and_then(Value::as_str).and_then(rfc3339),
        finished_at: st.get("FinishedAt").and_then(Value::as_str).and_then(rfc3339),
        exit_code: st.get("ExitCode").and_then(Value::as_i64),
        tty: v.get("Config").and_then(|c| c.get("Tty")).and_then(Value::as_bool).unwrap_or(false),
    })
}

/// One raw stats reading (CPU% needs two of them).
#[derive(Copy, Clone, Debug, Default, PartialEq, Serialize)]
pub struct StatsSample {
    /// Container CPU time, ns.
    pub cpu_total: u64,
    /// Host CPU time, ns (0 if the engine doesn't report it).
    pub system: u64,
    pub online_cpus: u32,
    /// Working-set memory (usage minus reclaimable page cache), bytes.
    pub mem_used: u64,
    pub mem_limit: u64,
}

pub fn parse_stats(body: &[u8]) -> Result<StatsSample, ApiError> {
    let v: Value = serde_json::from_slice(body).map_err(|e| ApiError::Protocol { message: e.to_string() })?;
    let cpu = v.get("cpu_stats").cloned().unwrap_or(Value::Null);
    let mem = v.get("memory_stats").cloned().unwrap_or(Value::Null);
    let n = |v: &Value, path: &[&str]| -> u64 {
        let mut cur = v;
        for k in path {
            match cur.get(*k) {
                Some(x) => cur = x,
                None => return 0,
            }
        }
        cur.as_u64().unwrap_or(0)
    };
    let percpu = cpu.get("cpu_usage").and_then(|c| c.get("percpu_usage")).and_then(Value::as_array).map(|a| a.len() as u32).unwrap_or(0);
    let online = match n(&cpu, &["online_cpus"]) as u32 {
        0 if percpu > 0 => percpu,
        0 => std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1),
        k => k,
    };
    let usage = n(&mem, &["usage"]);
    // cgroup v2: inactive_file; v1: total_inactive_file; very old: cache.
    let reclaimable = [["stats", "inactive_file"], ["stats", "total_inactive_file"], ["stats", "cache"]].iter().map(|p| n(&mem, p)).find(|x| *x > 0).unwrap_or(0);
    Ok(StatsSample {
        cpu_total: n(&cpu, &["cpu_usage", "total_usage"]),
        system: n(&cpu, &["system_cpu_usage"]),
        online_cpus: online,
        mem_used: usage.saturating_sub(reclaimable),
        mem_limit: n(&mem, &["limit"]),
    })
}

/// CPU use between two samples, `docker stats` style (100 % = one core).
pub fn cpu_percent(prev: &StatsSample, cur: &StatsSample) -> Option<f32> {
    let dcpu = cur.cpu_total.checked_sub(prev.cpu_total)?;
    let dsys = cur.system.checked_sub(prev.system)?;
    if dsys == 0 {
        return None;
    }
    Some((dcpu as f64 / dsys as f64 * cur.online_cpus.max(1) as f64 * 100.0) as f32)
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Stream {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LogLine {
    pub stream: Stream,
    pub text: String,
}

/// Whether `body` looks like Docker's multiplexed stdout/stderr framing.
fn is_multiplexed(body: &[u8]) -> bool {
    body.len() >= FRAME_HEADER && matches!(body[0], 0..=2) && body[1..4] == [0, 0, 0]
}

/// Splits a logs response into lines: demultiplexes the 8-byte frame
/// headers of non-TTY containers, or treats a TTY stream as stdout. Keeps
/// the last `tail` lines.
pub fn parse_logs(body: &[u8], tail: usize) -> Vec<LogLine> {
    let mut chunks: Vec<(Stream, Vec<u8>)> = Vec::new();
    if is_multiplexed(body) {
        let mut rest = body;
        while rest.len() >= FRAME_HEADER {
            let stream = if rest[0] == 2 { Stream::Stderr } else { Stream::Stdout };
            let len = u32::from_be_bytes([rest[4], rest[5], rest[6], rest[7]]) as usize;
            let end = (FRAME_HEADER + len).min(rest.len());
            let payload = rest[FRAME_HEADER..end].to_vec();
            match chunks.last_mut() {
                Some((s, buf)) if *s == stream && !buf.ends_with(b"\n") => buf.extend_from_slice(&payload),
                _ => chunks.push((stream, payload)),
            }
            rest = &rest[end..];
        }
    } else {
        chunks.push((Stream::Stdout, body.to_vec()));
    }
    let mut lines: Vec<LogLine> = Vec::new();
    for (stream, buf) in chunks {
        for line in String::from_utf8_lossy(&buf).split('\n') {
            let text = strip_ansi(line.trim_end_matches('\r'));
            lines.push(LogLine { stream, text });
        }
        // A trailing newline produces one empty tail element.
        if lines.last().is_some_and(|l| l.text.is_empty()) {
            lines.pop();
        }
    }
    let skip = lines.len().saturating_sub(tail);
    lines.split_off(skip)
}

/// Drops ANSI escape sequences (colours) from a log line.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                // CSI: parameters until a final byte in @..~
                for d in chars.by_ref() {
                    if ('@'..='~').contains(&d) {
                        break;
                    }
                }
            } else {
                chars.next();
            }
        } else if c != '\u{7}' {
            out.push(c);
        }
    }
    out
}

fn array_len(body: &[u8], key: Option<&str>) -> Result<usize, ApiError> {
    let v: Value = serde_json::from_slice(body).map_err(|e| ApiError::Protocol { message: e.to_string() })?;
    let v = match key {
        Some(k) => v.get(k).cloned().unwrap_or(Value::Null),
        None => v,
    };
    Ok(v.as_array().map(Vec::len).unwrap_or(0))
}

// ── Actions ──────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Start,
    Stop,
    Restart,
    Pause,
    Unpause,
}

impl Action {
    pub const ALL: [Action; 5] = [Action::Start, Action::Stop, Action::Restart, Action::Pause, Action::Unpause];

    pub fn verb(self) -> &'static str {
        match self {
            Action::Start => "start",
            Action::Stop => "stop",
            Action::Restart => "restart",
            Action::Pause => "pause",
            Action::Unpause => "unpause",
        }
    }

    pub fn parse(s: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|a| a.verb() == s)
    }

    /// Whether the action makes sense for a container in `state`.
    pub fn applies_to(self, state: State) -> bool {
        match self {
            Action::Start => matches!(state, State::Created | State::Exited | State::Dead | State::Unknown),
            Action::Stop | Action::Restart => matches!(state, State::Running | State::Restarting | State::Paused),
            Action::Pause => state == State::Running,
            Action::Unpause => state == State::Paused,
        }
    }
}

/// Container IDs and names: `[A-Za-z0-9][A-Za-z0-9_.-]*` (what engines allow).
pub fn valid_ref(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric()) && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// `(method, path)` of a lifecycle action.
pub fn action_request(id: &str, action: Action) -> Result<(&'static str, String), ApiError> {
    if !valid_ref(id) {
        return Err(ApiError::InvalidName { name: id.to_string() });
    }
    Ok(("POST", format!("/containers/{id}/{}", action.verb())))
}

// ── Client ───────────────────────────────────────────────────────────────

/// API calls on top of a [`Transport`].
pub struct Client<T: Transport> {
    pub transport: T,
}

impl Client<UnixTransport> {
    /// A client for `pref` on this machine.
    pub fn connect(pref: Preference) -> Result<(Endpoint, Self), ApiError> {
        let ep = choose(&discover(), pref).ok_or(ApiError::NoEngine)?;
        let client = Client { transport: UnixTransport::new(&ep.socket) };
        Ok((ep, client))
    }
}

impl<T: Transport> Client<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    fn get(&mut self, path: &str) -> Result<Response, ApiError> {
        let r = self.transport.request("GET", path)?;
        if r.ok() { Ok(r) } else { Err(ApiError::Http { status: r.status, message: r.error_message() }) }
    }

    pub fn version(&mut self) -> Result<String, ApiError> {
        let r = self.get("/version")?;
        let v: Value = serde_json::from_slice(&r.body).map_err(|e| ApiError::Protocol { message: e.to_string() })?;
        Ok(str_of(&v, "Version"))
    }

    pub fn list(&mut self) -> Result<Vec<Container>, ApiError> {
        parse_containers(&self.get("/containers/json?all=1")?.body)
    }

    pub fn inspect(&mut self, id: &str) -> Result<Inspect, ApiError> {
        if !valid_ref(id) {
            return Err(ApiError::InvalidName { name: id.to_string() });
        }
        parse_inspect(&self.get(&format!("/containers/{id}/json"))?.body)
    }

    pub fn stats(&mut self, id: &str) -> Result<StatsSample, ApiError> {
        if !valid_ref(id) {
            return Err(ApiError::InvalidName { name: id.to_string() });
        }
        parse_stats(&self.get(&format!("/containers/{id}/stats?stream=false&one-shot=true"))?.body)
    }

    pub fn logs(&mut self, id: &str, tail: usize) -> Result<Vec<LogLine>, ApiError> {
        if !valid_ref(id) {
            return Err(ApiError::InvalidName { name: id.to_string() });
        }
        let tail = tail.clamp(1, MAX_LOG_LINES);
        Ok(parse_logs(&self.get(&format!("/containers/{id}/logs?stdout=1&stderr=1&tail={tail}"))?.body, tail))
    }

    /// `(images, volumes)`.
    pub fn counts(&mut self) -> Result<(usize, usize), ApiError> {
        let images = array_len(&self.get("/images/json")?.body, None)?;
        let volumes = array_len(&self.get("/volumes")?.body, Some("Volumes"))?;
        Ok((images, volumes))
    }

    pub fn act(&mut self, id: &str, action: Action) -> Result<(), ApiError> {
        let (method, path) = action_request(id, action)?;
        let r = self.transport.request(method, &path)?;
        match r.status {
            // 304: already in the requested state.
            200..=299 | 304 => Ok(()),
            404 => Err(ApiError::NotFound { name: id.to_string() }),
            status => Err(ApiError::Http { status, message: r.error_message() }),
        }
    }

    /// Finds a container by exact name, ID or unique ID prefix.
    pub fn resolve(&mut self, name_or_id: &str) -> Result<Container, ApiError> {
        let key = name_or_id.trim().trim_start_matches('/');
        if !valid_ref(key) {
            return Err(ApiError::InvalidName { name: key.to_string() });
        }
        let all = self.list()?;
        if let Some(c) = all.iter().find(|c| c.name == key || c.id == key) {
            return Ok(c.clone());
        }
        let mut prefixed = all.into_iter().filter(|c| c.id.starts_with(key));
        match (prefixed.next(), prefixed.next()) {
            (Some(c), None) => Ok(c),
            _ => Err(ApiError::NotFound { name: key.to_string() }),
        }
    }
}

// ── Compose grouping ─────────────────────────────────────────────────────

/// A compose project (or a single loose container when `project` is `None`).
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub project: Option<String>,
    pub members: Vec<Container>,
}

impl Group {
    pub fn running(&self) -> usize {
        self.members.iter().filter(|c| c.state.is_up()).count()
    }
}

/// Groups containers by compose project; loose containers stay single.
/// Order: groups with anything up first, then by name.
pub fn group_by_project(containers: &[Container]) -> Vec<Group> {
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<Group> = Vec::new();
    for c in containers {
        match &c.project {
            Some(p) => match index.get(p) {
                Some(&i) => out[i].members.push(c.clone()),
                None => {
                    index.insert(p.clone(), out.len());
                    out.push(Group { project: Some(p.clone()), members: vec![c.clone()] });
                }
            },
            None => out.push(Group { project: None, members: vec![c.clone()] }),
        }
    }
    let key = |g: &Group| g.project.clone().unwrap_or_else(|| g.members[0].name.clone()).to_lowercase();
    out.sort_by(|a, b| (b.running() > 0).cmp(&(a.running() > 0)).then_with(|| key(a).cmp(&key(b))));
    out
}

// ── Monitor (shared poller) ──────────────────────────────────────────────

/// Live readings for one up container.
#[derive(Copy, Clone, Debug, Default, PartialEq, Serialize)]
pub struct Live {
    /// `None` until two samples exist.
    pub cpu: Option<f32>,
    pub mem_used: u64,
    pub mem_limit: u64,
}

/// What the poller knows. Cheap to clone.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// Engines present on this machine.
    pub endpoints: Vec<Endpoint>,
    /// The engine in use.
    pub endpoint: Option<Endpoint>,
    pub version: Option<String>,
    pub containers: Vec<Container>,
    pub inspect: HashMap<String, Inspect>,
    pub live: HashMap<String, Live>,
    pub images: Option<usize>,
    pub volumes: Option<usize>,
    pub error: Option<ApiError>,
    /// When the container list was last read successfully.
    pub listed: Option<Instant>,
}

impl Snapshot {
    pub fn running(&self) -> usize {
        self.containers.iter().filter(|c| c.state == State::Running).count()
    }
}

/// Reply channel for actions and log requests.
pub type ActReply = async_channel::Sender<Result<(), String>>;
pub type LogsReply = async_channel::Sender<Result<Vec<LogLine>, String>>;

enum Cmd {
    Wake,
    Preference(Preference),
    ListEvery(Duration),
    Use(Engine),
    Act { id: String, action: Action, reply: ActReply },
    Logs { id: String, tail: usize, reply: LogsReply },
}

/// Handle to the poller thread. Dropping every clone stops the thread.
#[derive(Clone)]
pub struct Monitor {
    cmd: mpsc::Sender<Cmd>,
    pub snapshot: Arc<Mutex<Snapshot>>,
    /// Ticks whenever the snapshot changes.
    pub changed: async_channel::Receiver<()>,
    detail_until: Arc<Mutex<Option<Instant>>>,
}

impl Monitor {
    /// Marks the panel as open for [`DETAIL_TTL`] (call while it renders).
    pub fn ping_detail(&self) {
        let mut d = self.detail_until.lock();
        let was_idle = d.is_none_or(|t| t <= Instant::now());
        *d = Some(Instant::now() + DETAIL_TTL);
        drop(d);
        if was_idle {
            let _ = self.cmd.send(Cmd::Wake);
        }
    }

    pub fn set_preference(&self, pref: Preference) {
        let _ = self.cmd.send(Cmd::Preference(pref));
    }

    /// Lists containers this often while the panel is closed (clamped to
    /// [`LIST_EVERY_RANGE`]).
    pub fn set_list_every(&self, every: Duration) {
        let _ = self.cmd.send(Cmd::ListEvery(every));
    }

    /// Switches engine for this session (the panel's engine switcher).
    pub fn use_engine(&self, engine: Engine) {
        let _ = self.cmd.send(Cmd::Use(engine));
    }

    pub fn refresh(&self) {
        let _ = self.cmd.send(Cmd::Wake);
    }

    pub fn act(&self, id: &str, action: Action) -> async_channel::Receiver<Result<(), String>> {
        let (tx, rx) = async_channel::bounded(1);
        if self.cmd.send(Cmd::Act { id: id.to_string(), action, reply: tx.clone() }).is_err() {
            let _ = tx.try_send(Err(t("The service is not running").into()));
        }
        rx
    }

    pub fn logs(&self, id: &str, tail: usize) -> async_channel::Receiver<Result<Vec<LogLine>, String>> {
        let (tx, rx) = async_channel::bounded(1);
        if self.cmd.send(Cmd::Logs { id: id.to_string(), tail, reply: tx.clone() }).is_err() {
            let _ = tx.try_send(Err(t("The service is not running").into()));
        }
        rx
    }
}

struct Poller {
    pref: Preference,
    /// Session override from the engine switcher.
    forced: Option<Engine>,
    snapshot: Arc<Mutex<Snapshot>>,
    changed: async_channel::Sender<()>,
    detail_until: Arc<Mutex<Option<Instant>>>,
    samples: HashMap<String, StatsSample>,
    counts_at: Option<Instant>,
    /// Detail mode on the previous tick (re-inspect on each panel open).
    was_detail: bool,
    /// List cadence while the panel is closed.
    list_every: Duration,
}

impl Poller {
    fn detail(&self) -> bool {
        self.detail_until.lock().is_some_and(|t| t > Instant::now())
    }

    fn client(&self) -> (Vec<Endpoint>, Option<Endpoint>) {
        let endpoints = discover();
        let pref = self.forced.map(Preference::Only).unwrap_or(self.pref);
        // A forced or preferred engine that vanished falls back to auto.
        let ep = choose(&endpoints, pref).or_else(|| choose(&endpoints, Preference::Auto));
        (endpoints, ep)
    }

    fn publish(&self) {
        let _ = self.changed.try_send(());
    }

    fn tick(&mut self) {
        let detail = self.detail();
        let reopened = detail && !self.was_detail;
        self.was_detail = detail;
        let (endpoints, ep) = self.client();
        let Some(ep) = ep else {
            let mut s = self.snapshot.lock();
            *s = Snapshot { endpoints, error: Some(ApiError::NoEngine), ..Snapshot::default() };
            drop(s);
            self.samples.clear();
            self.publish();
            return;
        };
        let switched = self.snapshot.lock().endpoint.as_ref() != Some(&ep);
        if switched {
            self.samples.clear();
            self.counts_at = None;
            *self.snapshot.lock() = Snapshot { endpoints: endpoints.clone(), endpoint: Some(ep.clone()), ..Snapshot::default() };
        }
        let mut client = Client::new(UnixTransport::new(&ep.socket));
        let containers = match client.list() {
            Ok(c) => c,
            Err(e) => {
                let mut s = self.snapshot.lock();
                s.endpoints = endpoints;
                s.endpoint = Some(ep);
                s.error = Some(e);
                drop(s);
                self.publish();
                return;
            }
        };
        let version = if switched || self.snapshot.lock().version.is_none() { client.version().ok() } else { None };
        {
            let mut s = self.snapshot.lock();
            s.endpoints = endpoints;
            s.endpoint = Some(ep);
            s.error = None;
            s.listed = Some(Instant::now());
            if version.is_some() {
                s.version = version;
            }
            // Forget readings of containers that went away or went down.
            let up: HashSet<&str> = containers.iter().filter(|c| c.state.is_up()).map(|c| c.id.as_str()).collect();
            s.live.retain(|id, _| up.contains(id.as_str()));
            let ids: HashSet<&str> = containers.iter().map(|c| c.id.as_str()).collect();
            s.inspect.retain(|id, _| ids.contains(id.as_str()));
            // A state change invalidates its start / finish times.
            let old: HashMap<String, State> = s.containers.iter().map(|c| (c.id.clone(), c.state)).collect();
            for c in &containers {
                if old.get(&c.id).is_some_and(|st| *st != c.state) {
                    s.inspect.remove(&c.id);
                }
            }
            s.containers = containers.clone();
        }
        self.samples.retain(|id, _| containers.iter().any(|c| c.id == *id && c.state == State::Running));
        self.publish();
        if !detail {
            return;
        }
        // Inspect (start / finish times) for containers we haven't yet.
        let missing: Vec<String> = {
            let s = self.snapshot.lock();
            containers.iter().filter(|c| reopened || !s.inspect.contains_key(&c.id)).map(|c| c.id.clone()).collect()
        };
        for id in missing {
            if let Ok(i) = client.inspect(&id) {
                self.snapshot.lock().inspect.insert(id, i);
            }
        }
        // Stats for running containers.
        for c in containers.iter().filter(|c| c.state == State::Running) {
            let Ok(cur) = client.stats(&c.id) else { continue };
            let cpu = self.samples.get(&c.id).and_then(|prev| cpu_percent(prev, &cur));
            self.samples.insert(c.id.clone(), cur);
            let mut s = self.snapshot.lock();
            let prev_cpu = s.live.get(&c.id).and_then(|l| l.cpu);
            s.live.insert(c.id.clone(), Live { cpu: cpu.or(prev_cpu), mem_used: cur.mem_used, mem_limit: cur.mem_limit });
        }
        if self.counts_at.is_none_or(|t| t.elapsed() >= COUNTS_EVERY)
            && let Ok((images, volumes)) = client.counts()
        {
            self.counts_at = Some(Instant::now());
            let mut s = self.snapshot.lock();
            s.images = Some(images);
            s.volumes = Some(volumes);
        }
        self.publish();
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Wake => {}
            Cmd::Preference(p) => {
                if p != self.pref {
                    self.pref = p;
                    self.forced = None;
                }
            }
            Cmd::ListEvery(d) => self.list_every = d.clamp(LIST_EVERY_RANGE.0, LIST_EVERY_RANGE.1),
            Cmd::Use(e) => self.forced = Some(e),
            Cmd::Act { id, action, reply } => {
                let ep = self.snapshot.lock().endpoint.clone();
                let result = match ep {
                    Some(ep) => Client::new(UnixTransport::new(&ep.socket)).act(&id, action).map_err(|e| e.message()),
                    None => Err(ApiError::NoEngine.message()),
                };
                self.snapshot.lock().inspect.remove(&id);
                let _ = reply.try_send(result);
            }
            Cmd::Logs { id, tail, reply } => {
                let ep = self.snapshot.lock().endpoint.clone();
                let result = match ep {
                    Some(ep) => Client::new(UnixTransport::new(&ep.socket)).logs(&id, tail).map_err(|e| e.message()),
                    None => Err(ApiError::NoEngine.message()),
                };
                let _ = reply.try_send(result);
            }
        }
    }
}

/// Starts the shared poller thread.
pub fn spawn_monitor(pref: Preference) -> Monitor {
    let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>();
    let (changed_tx, changed_rx) = async_channel::bounded(1);
    let snapshot = Arc::new(Mutex::new(Snapshot::default()));
    let detail_until = Arc::new(Mutex::new(None));
    let mut poller = Poller {
        pref,
        forced: None,
        snapshot: snapshot.clone(),
        changed: changed_tx,
        detail_until: detail_until.clone(),
        samples: HashMap::new(),
        counts_at: None,
        was_detail: false,
        list_every: LIST_EVERY,
    };
    let spawned = std::thread::Builder::new().name("mandara-containers".into()).spawn(move || {
        poller.tick();
        loop {
            let wait = if poller.detail() { DETAIL_EVERY } else { poller.list_every };
            let deadline = Instant::now() + wait;
            // Drain commands until the next tick is due.
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                match cmd_rx.recv_timeout(left) {
                    Ok(cmd) => {
                        let refresh = !matches!(cmd, Cmd::Logs { .. });
                        poller.handle(cmd);
                        if refresh {
                            break;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
            poller.tick();
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("containers poller: {e}");
    }
    Monitor { cmd: cmd_tx, snapshot, changed: changed_rx, detail_until }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records requests and replies with canned responses.
    struct Mock {
        sent: Vec<(String, String)>,
        replies: Vec<Response>,
    }

    impl Mock {
        fn new(replies: Vec<Response>) -> Self {
            Self { sent: Vec::new(), replies }
        }
    }

    impl Transport for Mock {
        fn request(&mut self, method: &str, path: &str) -> Result<Response, ApiError> {
            self.sent.push((method.to_string(), path.to_string()));
            if self.replies.is_empty() { Err(ApiError::Io { message: "no reply".into() }) } else { Ok(self.replies.remove(0)) }
        }
    }

    fn resp(status: u16, body: &str) -> Response {
        Response { status, headers: Vec::new(), body: body.as_bytes().to_vec() }
    }

    const DOCKER_LIST: &str = r#"[
      {"Id":"f12daf05f69c7b1c66120ccce1cd7e2285a969dbd2fc621ebb899353c5b5978c","Names":["/linkit-postgres"],"Image":"postgres:18",
       "Created":1793047629,"State":"running","Status":"Up 15 hours (healthy)",
       "Ports":[{"IP":"0.0.0.0","PrivatePort":5432,"PublicPort":55432,"Type":"tcp"},{"IP":"::","PrivatePort":5432,"PublicPort":55432,"Type":"tcp"}],
       "Labels":{"com.docker.compose.project":"linkit","com.docker.compose.service":"postgres"}},
      {"Id":"aa00","Names":["/old-job"],"Image":"alpine","Created":1700000000,"State":"exited","Status":"Exited (137) 3 days ago","Ports":[],"Labels":{}},
      {"Id":"bb11","Names":["/portainer"],"Image":"portainer/portainer-ce","Created":1700000001,"State":"paused","Status":"Up 2 hours (Paused)",
       "Ports":[{"IP":"","PrivatePort":9000,"Type":"tcp"}],"Labels":{}}
    ]"#;

    const PODMAN_LIST: &str = r#"[
      {"Id":"cc22","Names":["web"],"Image":"docker.io/library/nginx:latest","Created":1700000100,"State":"running","Status":"Up 2 minutes",
       "Ports":[{"host_ip":"","container_port":80,"host_port":8080,"protocol":"tcp"}],"Labels":{"io.podman.compose.project":"site"}},
      {"Id":"dd33","Names":["db"],"Image":"postgres","Created":1700000101,"State":"created","Status":"Created","Ports":null,"Labels":null}
    ]"#;

    #[test]
    fn parses_docker_list() {
        let cs = parse_containers(DOCKER_LIST.as_bytes()).unwrap();
        // Up (running / paused) first, then by name.
        assert_eq!(cs.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["linkit-postgres", "portainer", "old-job"]);
        let pg = &cs[0];
        assert_eq!(pg.state, State::Running);
        assert_eq!(pg.health, Some(Health::Healthy));
        assert_eq!(pg.project.as_deref(), Some("linkit"));
        assert_eq!(pg.service.as_deref(), Some("postgres"));
        assert_eq!(pg.unique_ports(), vec![Port { private: 5432, public: Some(55432), ip: "0.0.0.0".into(), proto: "tcp".into() }]);
        assert_eq!(pg.short_id(), "f12daf05f69c");
        assert_eq!(cs[1].state, State::Paused);
        assert_eq!(cs[2].exit_code, Some(137));
        assert!(pg.matches("LINKIT") && pg.matches("postgres:") && pg.matches("f12d") && !pg.matches("nginx"));
    }

    #[test]
    fn parses_podman_list_with_nulls() {
        let cs = parse_containers(PODMAN_LIST.as_bytes()).unwrap();
        assert_eq!(cs[0].name, "web");
        assert_eq!(cs[0].project.as_deref(), Some("site"));
        assert_eq!(cs[0].ports[0].public, Some(8080));
        assert_eq!(cs[1].state, State::Created);
        assert!(cs[1].ports.is_empty() && cs[1].project.is_none());
    }

    #[test]
    fn groups_compose_projects() {
        let mut cs = parse_containers(DOCKER_LIST.as_bytes()).unwrap();
        cs.extend(parse_containers(PODMAN_LIST.as_bytes()).unwrap());
        let gs = group_by_project(&cs);
        let keys: Vec<String> = gs.iter().map(|g| g.project.clone().unwrap_or_else(|| g.members[0].name.clone())).collect();
        // Groups with something up first (alphabetical), then the rest.
        assert_eq!(keys, ["linkit", "portainer", "site", "db", "old-job"]);
        assert_eq!(gs[0].running(), 1);
    }

    #[test]
    fn inspect_times() {
        let i = parse_inspect(br#"{"State":{"StartedAt":"2026-10-26T20:47:09.589549906Z","FinishedAt":"0001-01-01T00:00:00Z","ExitCode":0},"Config":{"Tty":true}}"#).unwrap();
        assert_eq!(i.started_at, Some(1793047629));
        assert_eq!(i.finished_at, None);
        assert!(i.tty);
    }

    #[test]
    fn stats_and_cpu_percent() {
        let a = parse_stats(br#"{"cpu_stats":{"cpu_usage":{"total_usage":1000000000},"system_cpu_usage":100000000000,"online_cpus":4},
            "memory_stats":{"usage":22265856,"stats":{"inactive_file":15196160},"limit":98702852096}}"#)
        .unwrap();
        assert_eq!(a.mem_used, 22265856 - 15196160);
        assert_eq!(a.online_cpus, 4);
        let b = StatsSample { cpu_total: a.cpu_total + 500_000_000, system: a.system + 4_000_000_000, ..a };
        // 0.5 s of CPU over 4 s of host time on 4 cores = 50 %.
        assert!((cpu_percent(&a, &b).unwrap() - 50.0).abs() < 0.01);
        assert_eq!(cpu_percent(&b, &a), None);
        // cgroup v1 field name.
        let v1 = parse_stats(br#"{"cpu_stats":{},"memory_stats":{"usage":1000,"stats":{"total_inactive_file":400}}}"#).unwrap();
        assert_eq!(v1.mem_used, 600);
    }

    #[test]
    fn demultiplexes_logs() {
        let mut body = Vec::new();
        let frame = |stream: u8, text: &str, out: &mut Vec<u8>| {
            out.extend_from_slice(&[stream, 0, 0, 0]);
            out.extend_from_slice(&(text.len() as u32).to_be_bytes());
            out.extend_from_slice(text.as_bytes());
        };
        frame(1, "one\ntwo\n", &mut body);
        frame(2, "\u{1b}[31mbad\u{1b}[0m\r\n", &mut body);
        frame(1, "three", &mut body);
        let lines = parse_logs(&body, 10);
        assert_eq!(lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["one", "two", "bad", "three"]);
        assert_eq!(lines[2].stream, Stream::Stderr);
        assert_eq!(parse_logs(&body, 2).len(), 2);
        // TTY containers: raw text.
        let tty = parse_logs(b"a\nb\n", 10);
        assert_eq!(tty.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    }

    #[test]
    fn http_parsing() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n[1,2\r\n2;ext=1\r\n,3\r\n1\r\n]\r\n0\r\n\r\n";
        assert!(is_complete(raw));
        let r = parse_response(raw).unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.body, b"[1,2,3]");
        assert!(!is_complete(&raw[..raw.len() - 4]));
        let cl = b"HTTP/1.1 404 Not Found\r\nContent-Length: 29\r\n\r\n{\"message\":\"page not found\"}\n";
        let r = parse_response(cl).unwrap();
        assert_eq!(r.status, 404);
        assert_eq!(r.error_message(), "page not found");
        assert!(is_complete(b"HTTP/1.1 204 No Content\r\nApi-Version: 1.56\r\n\r\n"));
        assert!(parse_response(b"garbage").is_err());
        let req = String::from_utf8(request_bytes("POST", "/containers/x/stop")).unwrap();
        assert!(req.starts_with("POST /containers/x/stop HTTP/1.1\r\nHost: localhost\r\n"));
        assert!(req.contains("Connection: close\r\n") && req.contains("Content-Length: 0\r\n") && req.ends_with("\r\n\r\n"));
        assert!(!String::from_utf8(request_bytes("GET", "/version")).unwrap().contains("Content-Length"));
    }

    #[test]
    fn mutations_send_the_right_requests() {
        let mut client = Client::new(Mock::new(vec![resp(204, ""), resp(304, ""), resp(404, r#"{"message":"No such container: nope"}"#), resp(500, r#"{"message":"boom"}"#)]));
        assert_eq!(client.act("linkit-postgres", Action::Stop), Ok(()));
        assert_eq!(client.act("f12daf05", Action::Start), Ok(()));
        assert_eq!(client.act("nope", Action::Restart), Err(ApiError::NotFound { name: "nope".into() }));
        assert_eq!(client.act("web", Action::Pause), Err(ApiError::Http { status: 500, message: "boom".into() }));
        // Path injection is refused before anything is sent.
        assert_eq!(client.act("../../images/x", Action::Unpause), Err(ApiError::InvalidName { name: "../../images/x".into() }));
        assert_eq!(
            client.transport.sent,
            vec![
                ("POST".into(), "/containers/linkit-postgres/stop".into()),
                ("POST".into(), "/containers/f12daf05/start".into()),
                ("POST".into(), "/containers/nope/restart".into()),
                ("POST".into(), "/containers/web/pause".into()),
            ]
        );
    }

    #[test]
    fn resolve_and_reads_use_get() {
        let mut client = Client::new(Mock::new(vec![resp(200, DOCKER_LIST), resp(200, DOCKER_LIST), resp(200, "[1,2,3]"), resp(200, r#"{"Volumes":null}"#)]));
        assert_eq!(client.resolve("/portainer").unwrap().id, "bb11");
        assert_eq!(client.resolve("f12d").unwrap().name, "linkit-postgres");
        assert_eq!(client.counts().unwrap(), (3, 0));
        assert!(client.transport.sent.iter().all(|(m, _)| m == "GET"));
        assert_eq!(client.transport.sent[2].1, "/images/json");
    }

    #[test]
    fn actions_and_refs() {
        assert!(Action::Start.applies_to(State::Exited) && !Action::Start.applies_to(State::Running));
        assert!(Action::Pause.applies_to(State::Running) && Action::Unpause.applies_to(State::Paused));
        assert!(Action::Stop.applies_to(State::Paused));
        assert_eq!(Action::parse("unpause"), Some(Action::Unpause));
        assert!(valid_ref("kaizen-db-1") && valid_ref("a.b_c") && !valid_ref("-x") && !valid_ref("a/b") && !valid_ref(""));
    }

    #[test]
    fn endpoint_discovery_order() {
        let c = candidates(Some("unix:///run/user/1000/podman/podman.sock"), Some(Path::new("/run/user/1000")), Some(Path::new("/home/u")));
        assert_eq!(c[0], Endpoint { engine: Engine::Podman, socket: "/run/user/1000/podman/podman.sock".into() });
        // The same socket isn't listed twice.
        assert_eq!(c.iter().filter(|e| e.socket.ends_with("podman/podman.sock") && e.socket.starts_with("/run/user")).count(), 1);
        assert!(c.iter().any(|e| e.socket == Path::new("/home/u/.docker/desktop/docker.sock")));
        let eps = vec![Endpoint { engine: Engine::Docker, socket: "/d".into() }, Endpoint { engine: Engine::Podman, socket: "/p".into() }];
        assert_eq!(choose(&eps, Preference::Auto).unwrap().engine, Engine::Docker);
        assert_eq!(choose(&eps, Preference::parse("podman")).unwrap().engine, Engine::Podman);
        assert_eq!(choose(&eps[..1], Preference::Only(Engine::Podman)), None);
    }

    #[test]
    fn exit_codes_and_health() {
        assert_eq!(exit_code_of("Exited (0) 2 hours ago"), Some(0));
        assert_eq!(exit_code_of("Up 2 hours"), None);
        assert_eq!(health_of("Up 1 minute (health: starting)"), Some(Health::Starting));
        assert_eq!(health_of("Up 3 days (unhealthy)"), Some(Health::Unhealthy));
        assert_eq!(strip_ansi("\u{1b}[1;32mok\u{1b}[0m done"), "ok done");
    }

    /// Read-only calls against the real engine (never mutates anything).
    #[test]
    #[ignore]
    fn live_read_only() {
        let Ok((ep, mut client)) = Client::connect(Preference::Auto) else {
            eprintln!("no engine socket");
            return;
        };
        eprintln!("{:?} {}", ep.engine, client.version().unwrap_or_default());
        let list = client.list().expect("list");
        eprintln!("{} containers, {} groups", list.len(), group_by_project(&list).len());
        eprintln!("images/volumes: {:?}", client.counts());
        if let Some(c) = list.iter().find(|c| c.state == State::Running) {
            let a = client.stats(&c.id).expect("stats");
            std::thread::sleep(Duration::from_millis(500));
            let b = client.stats(&c.id).expect("stats");
            eprintln!("{} cpu {:?}% mem {} / {}", c.name, cpu_percent(&a, &b), b.mem_used, b.mem_limit);
            eprintln!("inspect {:?}", client.inspect(&c.id));
            let logs = client.logs(&c.id, 5).expect("logs");
            eprintln!("last {} log lines, first: {:?}", logs.len(), logs.first());
            assert_eq!(client.resolve(&c.name).unwrap().id, c.id);
        }
    }
}
