//! A local Ollama server over its HTTP API: the version (`/api/version`),
//! models loaded in memory (`/api/ps`) with how much sits in VRAM, the
//! installed models (`/api/tags`), and loading / unloading a model through
//! `/api/generate` with a `keep_alive` and no prompt. Every call blocks;
//! callers run them off the UI thread.

use std::time::Duration;

use mandara_i18n::{t, tf};
use serde::{Deserialize, Serialize};

/// Where Ollama listens by default.
pub const DEFAULT_URL: &str = "http://127.0.0.1:11434";
/// Listing calls answer at once from a local server.
const LIST_TIMEOUT: Duration = Duration::from_secs(4);
/// Loading a large model from disk can take minutes.
const LOAD_TIMEOUT: Duration = Duration::from_secs(600);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long a warmed-up model stays loaded.
pub const WARM_KEEP_ALIVE: &str = "10m";
/// Longest error body quoted back.
const MAX_ERROR_CHARS: usize = 160;

/// A model in memory (`/api/ps`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Loaded {
    pub name: String,
    /// Bytes in memory in total.
    pub size: u64,
    /// Bytes of that in GPU memory.
    pub size_vram: u64,
    /// Unix seconds when it unloads (`None`: kept loaded).
    pub expires_at: Option<i64>,
    pub family: String,
    pub parameter_size: String,
    pub quantization: String,
}

impl Loaded {
    /// Share of the model in VRAM, `0..=1`.
    pub fn gpu_share(&self) -> f32 {
        if self.size == 0 { 0.0 } else { (self.size_vram as f64 / self.size as f64).clamp(0.0, 1.0) as f32 }
    }
}

/// An installed model (`/api/tags`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Installed {
    pub name: String,
    /// Bytes on disk.
    pub size: u64,
    pub parameter_size: String,
    pub quantization: String,
    pub family: String,
    /// Unix seconds.
    pub modified_at: Option<i64>,
}

/// Everything the panel shows, read in one go.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Snapshot {
    pub version: String,
    pub loaded: Vec<Loaded>,
    pub installed: Vec<Installed>,
}

impl Snapshot {
    /// Bytes in VRAM over every loaded model.
    pub fn vram(&self) -> u64 {
        self.loaded.iter().map(|m| m.size_vram).sum()
    }

    pub fn is_loaded(&self, name: &str) -> bool {
        self.loaded.iter().any(|m| m.name == name)
    }
}

#[derive(Deserialize, Default)]
struct RawDetails {
    #[serde(default)]
    family: String,
    #[serde(default)]
    parameter_size: String,
    #[serde(default)]
    quantization_level: String,
}

#[derive(Deserialize)]
struct RawLoaded {
    #[serde(default)]
    name: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    size_vram: u64,
    #[serde(default)]
    expires_at: String,
    #[serde(default)]
    details: RawDetails,
}

#[derive(Deserialize)]
struct RawInstalled {
    #[serde(default)]
    name: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    modified_at: String,
    #[serde(default)]
    details: RawDetails,
}

#[derive(Deserialize)]
struct RawList<T> {
    #[serde(default = "Option::default")]
    models: Option<Vec<T>>,
}

/// Times beyond this many years ahead mean "kept loaded" (`keep_alive: -1`).
const FOREVER_YEARS: i64 = 100;
const SECS_PER_YEAR: i64 = 365 * 24 * 3600;

fn unix(s: &str) -> Option<i64> {
    let at = chrono::DateTime::parse_from_rfc3339(s).ok()?.timestamp();
    (at > 0).then_some(at)
}

fn pick(name: String, model: String) -> String {
    if name.is_empty() { model } else { name }
}

fn bad_json(e: serde_json::Error) -> String {
    tf("Unexpected answer from Ollama: {error}", &[("error", &e)])
}

/// Parses `/api/version`.
pub fn parse_version(json: &str) -> Result<String, String> {
    #[derive(Deserialize)]
    struct V {
        #[serde(default)]
        version: String,
    }
    serde_json::from_str::<V>(json).map(|v| v.version).map_err(bad_json)
}

/// Parses `/api/ps`, largest first.
pub fn parse_loaded(json: &str, now: i64) -> Result<Vec<Loaded>, String> {
    let raw: RawList<RawLoaded> = serde_json::from_str(json).map_err(bad_json)?;
    let mut out: Vec<Loaded> = raw
        .models
        .unwrap_or_default()
        .into_iter()
        .map(|m| Loaded {
            name: pick(m.name, m.model),
            size: m.size,
            size_vram: m.size_vram.min(m.size.max(m.size_vram)),
            expires_at: unix(&m.expires_at).filter(|at| *at < now + FOREVER_YEARS * SECS_PER_YEAR),
            family: m.details.family,
            parameter_size: m.details.parameter_size,
            quantization: m.details.quantization_level,
        })
        .collect();
    out.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
    Ok(out)
}

/// Parses `/api/tags`, by name.
pub fn parse_installed(json: &str) -> Result<Vec<Installed>, String> {
    let raw: RawList<RawInstalled> = serde_json::from_str(json).map_err(bad_json)?;
    let mut out: Vec<Installed> = raw
        .models
        .unwrap_or_default()
        .into_iter()
        .map(|m| Installed {
            name: pick(m.name, m.model),
            size: m.size,
            parameter_size: m.details.parameter_size,
            quantization: m.details.quantization_level,
            family: m.details.family,
            modified_at: unix(&m.modified_at),
        })
        .collect();
    out.sort_by_key(|m| m.name.to_lowercase());
    Ok(out)
}

/// `url` without a trailing slash (the default when blank).
pub fn base(url: &str) -> String {
    let u = url.trim().trim_end_matches('/');
    if u.is_empty() { DEFAULT_URL.into() } else { u.to_string() }
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder().timeout_connect(Some(CONNECT_TIMEOUT)).timeout_global(Some(timeout)).http_status_as_error(false).build().into()
}

fn unreachable(base: &str, e: impl std::fmt::Display) -> String {
    tf("Ollama isn't reachable at {url} ({error})", &[("url", &base), ("error", &e)])
}

/// The body of a reply, or the server's error message.
fn body(mut reply: ureq::http::Response<ureq::Body>) -> Result<String, String> {
    let code = reply.status().as_u16();
    let text = reply.body_mut().read_to_string().map_err(|e| e.to_string())?;
    if (200..300).contains(&code) {
        return Ok(text);
    }
    #[derive(Deserialize)]
    struct E {
        error: String,
    }
    let msg = serde_json::from_str::<E>(&text).map(|e| e.error).unwrap_or(text);
    let msg: String = msg.trim().chars().take(MAX_ERROR_CHARS).collect();
    Err(if msg.is_empty() { tf("Ollama answered HTTP {code}", &[("code", &code)]) } else { msg })
}

fn get(agent: &ureq::Agent, base: &str, path: &str) -> Result<String, String> {
    let reply = agent.get(format!("{base}{path}")).call().map_err(|e| unreachable(base, e))?;
    body(reply)
}

/// Reads the version, loaded and installed models (blocking).
pub fn fetch(url: &str) -> Result<Snapshot, String> {
    let base = base(url);
    let agent = agent(LIST_TIMEOUT);
    let version = parse_version(&get(&agent, &base, "/api/version")?)?;
    let now = chrono::Utc::now().timestamp();
    let loaded = parse_loaded(&get(&agent, &base, "/api/ps")?, now)?;
    let installed = parse_installed(&get(&agent, &base, "/api/tags")?)?;
    Ok(Snapshot { version, loaded, installed })
}

fn generate(url: &str, model: &str, keep_alive: serde_json::Value, timeout: Duration) -> Result<(), String> {
    if model.trim().is_empty() {
        return Err(t("Name a model").into());
    }
    let base = base(url);
    let payload = serde_json::json!({ "model": model, "keep_alive": keep_alive }).to_string();
    let reply = agent(timeout).post(format!("{base}/api/generate")).header("Content-Type", "application/json").send(payload.as_str()).map_err(|e| unreachable(&base, e))?;
    body(reply).map(drop)
}

/// Loads `model` into memory and keeps it for [`WARM_KEEP_ALIVE`] (blocking;
/// a large model can take a while).
pub fn load(url: &str, model: &str) -> Result<(), String> {
    generate(url, model, serde_json::Value::from(WARM_KEEP_ALIVE), LOAD_TIMEOUT)
}

/// Unloads `model` from memory now.
pub fn unload(url: &str, model: &str) -> Result<(), String> {
    generate(url, model, serde_json::Value::from(0), LIST_TIMEOUT)
}

/// The installed model `query` names: exact, then without `:latest`, then
/// the only name starting with it.
pub fn resolve<'a>(names: impl IntoIterator<Item = &'a str>, query: &str) -> Result<String, String> {
    let names: Vec<&str> = names.into_iter().collect();
    let q = query.trim();
    if let Some(n) = names.iter().find(|n| **n == q || n.strip_suffix(":latest") == Some(q)) {
        return Ok(n.to_string());
    }
    let starts: Vec<&&str> = names.iter().filter(|n| n.starts_with(q)).collect();
    match starts.as_slice() {
        [one] => Ok(one.to_string()),
        [] => Err(tf("No model named {model}", &[("model", &q)])),
        _ => Err(tf("{model} matches several models: {names}", &[("model", &q), ("names", &starts.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(", "))])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS: &str = r#"{"models":[
      {"name":"llama3.2:3b","model":"llama3.2:3b","size":3400000000,"digest":"a80c","details":{"parent_model":"","format":"gguf","family":"llama","families":["llama"],"parameter_size":"3.2B","quantization_level":"Q4_K_M"},"expires_at":"2026-11-02T10:15:00.123456789+02:00","size_vram":3400000000,"context_length":4096},
      {"name":"qwen2.5:32b","model":"qwen2.5:32b","size":24000000000,"details":{"family":"qwen2","parameter_size":"32.8B","quantization_level":"Q4_K_M"},"expires_at":"2318-02-21T20:00:00Z","size_vram":9120000000}
    ]}"#;

    const TAGS: &str = r#"{"models":[
      {"name":"qwen2.5:32b","model":"qwen2.5:32b","modified_at":"2026-09-01T18:03:44.123+03:00","size":19851337640,"digest":"9f13","details":{"parent_model":"","format":"gguf","family":"qwen2","families":["qwen2"],"parameter_size":"32.8B","quantization_level":"Q4_K_M"}},
      {"name":"Llama3.2:3b","model":"Llama3.2:3b","modified_at":"2026-08-14T09:00:00Z","size":2019393189,"details":{"family":"llama","parameter_size":"3.2B","quantization_level":"Q4_K_M"}},
      {"model":"nomic-embed-text:latest","size":274302450,"details":{"family":"nomic-bert","parameter_size":"137M","quantization_level":"F16"}}
    ]}"#;

    #[test]
    fn maps_loaded_models_and_their_vram() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-02T08:00:00Z").unwrap().timestamp();
        let loaded = parse_loaded(PS, now).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].name, "qwen2.5:32b", "largest first");
        assert!((loaded[0].gpu_share() - 0.38).abs() < 0.001);
        assert_eq!(loaded[0].expires_at, None, "a keep-alive of -1 never expires");
        assert_eq!(loaded[0].parameter_size, "32.8B");
        let small = &loaded[1];
        assert_eq!(small.gpu_share(), 1.0);
        assert_eq!(small.expires_at, Some(chrono::DateTime::parse_from_rfc3339("2026-11-02T08:15:00Z").unwrap().timestamp()));
        assert_eq!(small.quantization, "Q4_K_M");
        let snap = Snapshot { version: "0.12.3".into(), loaded, installed: Vec::new() };
        assert_eq!(snap.vram(), 9_120_000_000 + 3_400_000_000);
        assert!(snap.is_loaded("llama3.2:3b"));
    }

    #[test]
    fn maps_installed_models() {
        let installed = parse_installed(TAGS).unwrap();
        let names: Vec<&str> = installed.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["Llama3.2:3b", "nomic-embed-text:latest", "qwen2.5:32b"]);
        assert_eq!(installed[1].family, "nomic-bert");
        assert_eq!(installed[1].modified_at, None);
        assert_eq!(installed[2].size, 19_851_337_640);
        assert_eq!(installed[2].modified_at, Some(chrono::DateTime::parse_from_rfc3339("2026-09-01T15:03:44Z").unwrap().timestamp()));
        assert_eq!(parse_version(r#"{"version":"0.12.3"}"#).unwrap(), "0.12.3");
        assert!(parse_installed("<html>").is_err());
        assert!(parse_loaded(r#"{"models":null}"#, 0).unwrap().is_empty());
        assert!(parse_loaded("{}", 0).unwrap().is_empty());
    }

    #[test]
    fn resolves_model_names() {
        let names = ["llama3.2:3b", "llama3.2:1b", "nomic-embed-text:latest"];
        assert_eq!(resolve(names, "nomic-embed-text").unwrap(), "nomic-embed-text:latest");
        assert_eq!(resolve(names, "llama3.2:1b").unwrap(), "llama3.2:1b");
        assert_eq!(resolve(names, "nomic").unwrap(), "nomic-embed-text:latest");
        assert!(resolve(names, "llama3.2").is_err(), "ambiguous");
        assert!(resolve(names, "mistral").is_err());
        assert_eq!(base(" http://box:11434/ "), "http://box:11434");
        assert_eq!(base(""), DEFAULT_URL);
    }
}
