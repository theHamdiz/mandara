//! GitHub Actions runs for the `ci` plugin: the latest workflow runs of a
//! few repositories from `GET /repos/{owner}/{repo}/actions/runs`.
//!
//! Requests carry a token when one is available (the `gh` CLI's
//! `gh auth token` when allowed, else `GITHUB_TOKEN`) for private
//! repositories and the higher rate limit; otherwise they go out
//! anonymously. The token stays in memory, is never logged, and is read
//! from `gh` at most every few minutes. Each repository's `ETag` is kept so
//! an unchanged list costs a `304`. Blocking: call it off the UI thread.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::DateTime;
use mandara_i18n::{t, tf};
use serde::{Deserialize, Serialize};
use ureq::http::StatusCode;

/// Runs fetched per repository.
pub const RUNS_PER_REPO: u32 = 5;
const API: &str = "https://api.github.com";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// Largest response body read.
const MAX_JSON_BYTES: u64 = 2 << 20;
/// `gh auth token` is asked again after this long.
const TOKEN_TTL: Duration = Duration::from_secs(600);
/// Longest repository part accepted (GitHub's own limit is 100).
const MAX_NAME_LEN: usize = 100;

/// Where the request's credentials came from (never the token itself).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthSource {
    GhCli,
    Env,
    Anonymous,
}

/// A token and where it came from. Deliberately not `Debug`.
#[derive(Clone)]
pub struct Auth {
    token: Option<String>,
    pub source: AuthSource,
}

impl Auth {
    pub fn anonymous() -> Self {
        Self { token: None, source: AuthSource::Anonymous }
    }
}

/// `gh auth token`, cached for [`TOKEN_TTL`].
fn gh_token() -> Option<String> {
    static CACHE: Mutex<Option<(Option<String>, Instant)>> = Mutex::new(None);
    if let Ok(cache) = CACHE.lock()
        && let Some((token, at)) = cache.as_ref()
        && at.elapsed() < TOKEN_TTL
    {
        return token.clone();
    }
    let gh = crate::updates::find_program("gh").or_else(|| dirs::home_dir().map(|h| h.join(".local/bin/gh")).filter(|p| p.is_file()))?;
    let token = std::process::Command::new(gh)
        .args(["auth", "token"])
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|t| !t.is_empty() && !t.contains(char::is_whitespace));
    if let Ok(mut cache) = CACHE.lock() {
        *cache = Some((token.clone(), Instant::now()));
    }
    token
}

/// The credentials to use: `gh` (when `use_gh`), else `GITHUB_TOKEN`, else
/// none.
pub fn auth(use_gh: bool) -> Auth {
    if use_gh && let Some(token) = gh_token() {
        return Auth { token: Some(token), source: AuthSource::GhCli };
    }
    match std::env::var("GITHUB_TOKEN").ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
        Some(token) => Auth { token: Some(token), source: AuthSource::Env },
        None => Auth::anonymous(),
    }
}

/// Where a run is.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    /// Queued, waiting or in progress.
    Running,
    Success,
    Failure,
    /// Cancelled, skipped or neutral: finished without a verdict.
    Cancelled,
}

/// One workflow run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Run {
    pub id: u64,
    pub workflow: String,
    /// The commit title (or the PR title for pull-request runs).
    pub title: String,
    pub branch: String,
    pub event: String,
    pub run_number: u64,
    /// GitHub's `status` (`queued`, `in_progress`, `completed`, …).
    pub status: String,
    /// GitHub's `conclusion` once completed (`success`, `failure`, …).
    pub conclusion: Option<String>,
    pub phase: Phase,
    pub html_url: String,
    /// Unix seconds.
    pub created_at: i64,
    pub started_at: i64,
    pub updated_at: i64,
}

impl Run {
    /// Seconds it took (finished) or has taken so far (running, at `now`).
    pub fn duration(&self, now: i64) -> i64 {
        let end = if self.phase == Phase::Running { now } else { self.updated_at };
        (end - self.started_at).max(0)
    }

    pub fn finished(&self) -> bool {
        self.phase != Phase::Running
    }
}

fn phase(status: &str, conclusion: Option<&str>) -> Phase {
    if status != "completed" {
        return Phase::Running;
    }
    match conclusion {
        Some("success") => Phase::Success,
        Some("failure" | "timed_out" | "startup_failure" | "action_required") => Phase::Failure,
        _ => Phase::Cancelled,
    }
}

#[derive(Deserialize)]
struct RawRuns {
    #[serde(default)]
    workflow_runs: Vec<RawRun>,
}

#[derive(Deserialize)]
struct RawCommit {
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct RawRun {
    id: u64,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    display_title: Option<String>,
    #[serde(default)]
    head_branch: Option<String>,
    #[serde(default)]
    event: String,
    #[serde(default)]
    run_number: u64,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    run_started_at: Option<String>,
    #[serde(default)]
    head_commit: Option<RawCommit>,
}

fn unix(s: Option<&str>) -> Option<i64> {
    DateTime::parse_from_rfc3339(s?).ok().map(|d| d.timestamp())
}

/// Maps the API's JSON to runs, newest first.
pub fn parse_runs(json: &str) -> Result<Vec<Run>, String> {
    let raw: RawRuns = serde_json::from_str(json).map_err(|e| tf("GitHub sent something unexpected: {error}", &[("error", &e)]))?;
    let mut runs: Vec<Run> = raw
        .workflow_runs
        .into_iter()
        .map(|r| {
            let status = r.status.unwrap_or_else(|| "queued".into());
            let created_at = unix(r.created_at.as_deref()).unwrap_or(0);
            let title = r
                .display_title
                .filter(|s| !s.trim().is_empty())
                .or_else(|| r.head_commit.map(|c| c.message.lines().next().unwrap_or_default().to_string()))
                .unwrap_or_default();
            Run {
                id: r.id,
                workflow: r.name.unwrap_or_default(),
                title,
                branch: r.head_branch.unwrap_or_default(),
                event: r.event,
                run_number: r.run_number,
                phase: phase(&status, r.conclusion.as_deref()),
                status,
                conclusion: r.conclusion,
                html_url: r.html_url,
                created_at,
                started_at: unix(r.run_started_at.as_deref()).unwrap_or(created_at),
                updated_at: unix(r.updated_at.as_deref()).unwrap_or(created_at),
            }
        })
        .collect();
    runs.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
    Ok(runs)
}

/// Whether `repo` looks like `owner/name` (letters, digits, `-`, `_`, `.`).
pub fn valid_repo(repo: &str) -> bool {
    let part = |s: &str| !s.is_empty() && s.len() <= MAX_NAME_LEN && s != "." && s != ".." && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    matches!(repo.split_once('/'), Some((owner, name)) if part(owner) && part(name))
}

/// The comma-separated `repos` setting → valid `owner/name`s (a pasted
/// `https://github.com/owner/name` works too), without duplicates.
pub fn parse_repos(setting: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in setting.split([',', '\n', ' ']) {
        let item = item.trim().trim_start_matches("https://").trim_start_matches("github.com/").trim_end_matches('/').trim_end_matches(".git");
        if valid_repo(item) && !out.iter().any(|r| r.eq_ignore_ascii_case(item)) {
            out.push(item.to_string());
        }
    }
    out
}

/// The result of one request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fetch {
    /// The list since the given `ETag` is unchanged.
    NotModified,
    Runs { runs: Vec<Run>, etag: Option<String> },
    /// Rate limited: try again after this many seconds.
    RateLimited(u64),
    Failed(String),
}

/// The latest runs of `repo` (`owner/name`). `etag` is the previous
/// answer's, for a cheap `304` when nothing changed.
pub fn fetch(repo: &str, auth: &Auth, etag: Option<&str>) -> Fetch {
    if !valid_repo(repo) {
        return Fetch::Failed(tf("“{repo}” isn't an owner/name repository", &[("repo", &repo)]));
    }
    if let Err(e) = crate::offline::check() {
        return Fetch::Failed(e);
    }
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(REQUEST_TIMEOUT)).http_status_as_error(false).build().into();
    let url = format!("{API}/repos/{repo}/actions/runs?per_page={RUNS_PER_REPO}");
    let mut req = agent
        .get(&url)
        .header("User-Agent", concat!("mandara-shell/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    if let Some(token) = &auth.token {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    if let Some(etag) = etag {
        req = req.header("If-None-Match", etag);
    }
    let mut r = match req.call() {
        Ok(r) => r,
        Err(e) => return Fetch::Failed(tf("Couldn't reach GitHub: {error}", &[("error", &e)])),
    };
    if let Some(seconds) = crate::marketplace::rate_limit_wait(&r) {
        return Fetch::RateLimited(seconds);
    }
    match r.status() {
        StatusCode::NOT_MODIFIED => return Fetch::NotModified,
        StatusCode::NOT_FOUND => {
            let why = if auth.token.is_some() { t("Not found: check the name, and that your token can see it") } else { t("Not found: check the name; private repositories need the gh CLI signed in or GITHUB_TOKEN") };
            return Fetch::Failed(why.into());
        }
        StatusCode::UNAUTHORIZED => return Fetch::Failed(t("GitHub refused the token: sign in again with gh auth login").into()),
        s if !s.is_success() => return Fetch::Failed(tf("GitHub answered {status}", &[("status", &s)])),
        _ => {}
    }
    let etag = r.headers().get("etag").and_then(|v| v.to_str().ok()).map(str::to_string);
    let body = match r.body_mut().with_config().limit(MAX_JSON_BYTES).read_to_string() {
        Ok(b) => b,
        Err(e) => return Fetch::Failed(tf("Couldn't reach GitHub: {error}", &[("error", &e)])),
    };
    match parse_runs(&body) {
        Ok(runs) => Fetch::Runs { runs, etag },
        Err(e) => Fetch::Failed(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a real `actions/runs` response.
    const SAMPLE: &str = r#"{
  "total_count": 1243,
  "workflow_runs": [
    {
      "id": 11834567012,
      "name": "CI",
      "node_id": "WFR_kwLOK",
      "head_branch": "main",
      "head_sha": "8a0c55b3e1f",
      "path": ".github/workflows/ci.yml",
      "display_title": "feat: round the screen corners by default (#19)",
      "run_number": 412,
      "event": "push",
      "status": "completed",
      "conclusion": "success",
      "workflow_id": 7712345,
      "html_url": "https://github.com/theHamdiz/mandara/actions/runs/11834567012",
      "created_at": "2026-10-30T09:12:04Z",
      "updated_at": "2026-10-30T09:19:31Z",
      "run_attempt": 1,
      "run_started_at": "2026-10-30T09:12:04Z",
      "head_commit": { "id": "8a0c55b3e1f", "message": "feat: round the screen corners by default (#19)\n\nLonger body", "author": { "name": "Ahmad Hamdi" } }
    },
    {
      "id": 11834590001,
      "name": "Release",
      "head_branch": "v0.6.0",
      "display_title": "chore(release): mandara v0.6.0",
      "run_number": 37,
      "event": "push",
      "status": "in_progress",
      "conclusion": null,
      "html_url": "https://github.com/theHamdiz/mandara/actions/runs/11834590001",
      "created_at": "2026-10-30T10:00:00Z",
      "updated_at": "2026-10-30T10:03:00Z",
      "run_started_at": "2026-10-30T10:00:05Z",
      "head_commit": { "message": "chore(release): mandara v0.6.0" }
    },
    {
      "id": 11834500000,
      "name": "CI",
      "head_branch": "fix/tray",
      "display_title": "",
      "run_number": 411,
      "event": "pull_request",
      "status": "completed",
      "conclusion": "timed_out",
      "html_url": "https://github.com/theHamdiz/mandara/actions/runs/11834500000",
      "created_at": "2026-10-29T08:00:00Z",
      "updated_at": "2026-10-29T09:00:00Z",
      "run_started_at": "2026-10-29T08:00:30Z",
      "head_commit": { "message": "fix(tray): menus on the right monitor\n\ndetails" }
    },
    {
      "id": 11834400000,
      "name": "Docs",
      "head_branch": "main",
      "run_number": 9,
      "event": "workflow_dispatch",
      "status": "completed",
      "conclusion": "cancelled",
      "html_url": "https://github.com/theHamdiz/mandara/actions/runs/11834400000",
      "created_at": "2026-10-28T08:00:00Z",
      "updated_at": "2026-10-28T08:01:00Z"
    }
  ]
}"#;

    #[test]
    fn maps_the_api_json_to_runs() {
        let runs = parse_runs(SAMPLE).unwrap();
        assert_eq!(runs.iter().map(|r| r.id).collect::<Vec<_>>(), [11834590001, 11834567012, 11834500000, 11834400000], "newest first");
        let release = &runs[0];
        assert_eq!((release.phase, release.workflow.as_str(), release.branch.as_str()), (Phase::Running, "Release", "v0.6.0"));
        assert_eq!(release.conclusion, None);
        assert_eq!(release.duration(release.started_at + 90), 90, "a running run's duration grows with now");
        let ci = &runs[1];
        assert_eq!((ci.phase, ci.run_number, ci.title.as_str()), (Phase::Success, 412, "feat: round the screen corners by default (#19)"));
        assert_eq!(ci.duration(i64::MAX), 7 * 60 + 27, "updated − started for a finished run");
        assert_eq!(ci.html_url, "https://github.com/theHamdiz/mandara/actions/runs/11834567012");
        let timed_out = &runs[2];
        assert_eq!(timed_out.phase, Phase::Failure);
        assert_eq!(timed_out.title, "fix(tray): menus on the right monitor", "an empty display title falls back to the commit's first line");
        let docs = &runs[3];
        assert_eq!((docs.phase, docs.started_at), (Phase::Cancelled, docs.created_at), "a missing start time falls back to creation");
        assert!(docs.finished() && !release.finished());
    }

    #[test]
    fn rejects_junk() {
        assert!(parse_runs("<html>").is_err());
        assert!(parse_runs("{}").unwrap().is_empty());
    }

    #[test]
    fn phases_follow_status_and_conclusion() {
        assert_eq!(phase("queued", None), Phase::Running);
        assert_eq!(phase("waiting", None), Phase::Running);
        assert_eq!(phase("completed", Some("startup_failure")), Phase::Failure);
        assert_eq!(phase("completed", Some("skipped")), Phase::Cancelled);
        assert_eq!(phase("completed", None), Phase::Cancelled);
    }

    #[test]
    fn repos_setting_is_validated() {
        assert_eq!(parse_repos("theHamdiz/mandara, https://github.com/rust-lang/rust.git, bad, a/b/c, ../x, theHamdiz/Mandara"), ["theHamdiz/mandara", "rust-lang/rust"]);
        assert!(valid_repo("o/n.rs") && !valid_repo("o/") && !valid_repo("o/n?x=1") && !valid_repo("./x"));
    }
}
