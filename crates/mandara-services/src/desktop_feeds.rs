//! Data for the desktop widgets that read beyond the shell: GitHub's
//! public contribution calendar and exchange rates (network, only when a
//! widget asks and you switched it on), your Steam library and Claude
//! Code's local usage logs (files on this computer, read on request).
//!
//! Everything here is blocking and runs on a background thread; the
//! caller caches results (on disk for the network ones) and asks again
//! only after their time to live.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Local, NaiveDate};
use mandara_i18n::tf;

/// Whole-request timeout.
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
/// GitHub user names: letters, digits and single hyphens, at most 39.
const GITHUB_NAME_MAX: usize = 39;
/// ISO 4217 codes are three letters.
const CURRENCY_CODE_LEN: usize = 3;

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(HTTP_TIMEOUT)).build().into()
}

fn get(url: &str) -> Result<String, String> {
    crate::offline::check()?;
    let mut response = agent().get(url).header("User-Agent", concat!("mandara-shell/", env!("CARGO_PKG_VERSION"))).call().map_err(|e| tf("Request failed: {error}", &[("error", &e)]).to_string())?;
    response.body_mut().read_to_string().map_err(|e| tf("Request failed: {error}", &[("error", &e)]).to_string())
}

/// Where the network widgets keep their last answers.
pub fn cache_dir() -> PathBuf {
    mandara_config::paths::cache_dir().join("desktop-feeds")
}

// ── GitHub ───────────────────────────────────────────────────────────────

/// One day of a contribution calendar: its level (0–4, as GitHub shades
/// it) and its count, when the page says.
#[derive(Clone, Debug, PartialEq)]
pub struct ContributionDay {
    pub date: NaiveDate,
    pub level: u8,
    pub count: u32,
}

/// Whether `user` can be a GitHub login (nothing else reaches the URL).
pub fn valid_github_user(user: &str) -> bool {
    !user.is_empty() && user.len() <= GITHUB_NAME_MAX && user.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && !user.starts_with('-') && !user.ends_with('-') && !user.contains("--")
}

/// The public contribution calendar of `user` (the last year, oldest
/// first).
pub fn github_contributions(user: &str) -> Result<Vec<ContributionDay>, String> {
    if !valid_github_user(user) {
        return Err(tf("“{user}” isn't a GitHub user name", &[("user", &user)]).to_string());
    }
    let html = get(&format!("https://github.com/users/{user}/contributions"))?;
    let days = parse_contributions(&html);
    if days.is_empty() { Err(tf("No contributions found for {user}", &[("user", &user)]).to_string()) } else { Ok(days) }
}

/// The value of `attr="…"` inside `tag`.
fn attr<'a>(tag: &'a str, attr: &str) -> Option<&'a str> {
    let key = format!("{attr}=\"");
    let start = tag.find(&key)? + key.len();
    tag[start..].split('"').next()
}

/// Days of GitHub's contribution calendar markup: every element with
/// `data-date` and `data-level`; counts from the tooltips (`<tool-tip
/// for="ID">5 contributions on …</tool-tip>`) when present.
pub fn parse_contributions(html: &str) -> Vec<ContributionDay> {
    let mut counts: HashMap<&str, u32> = HashMap::new();
    for part in html.split("<tool-tip").skip(1) {
        let Some(id) = attr(part, "for") else { continue };
        let text = part.split('>').nth(1).unwrap_or_default();
        let n = text.split_whitespace().next().and_then(|w| w.parse::<u32>().ok()).unwrap_or(0);
        counts.insert(id, n);
    }
    let mut days = Vec::new();
    for part in html.split('<').filter(|p| p.contains("data-date=") && p.contains("data-level=")) {
        let tag = part.split('>').next().unwrap_or(part);
        let (Some(date), Some(level)) = (attr(tag, "data-date").and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()), attr(tag, "data-level").and_then(|l| l.parse::<u8>().ok())) else { continue };
        let count = attr(tag, "id").and_then(|id| counts.get(id).copied()).unwrap_or(0);
        days.push(ContributionDay { date, level: level.min(4), count });
    }
    days.sort_by_key(|d| d.date);
    days.dedup_by_key(|d| d.date);
    days
}

// ── Exchange rates ───────────────────────────────────────────────────────

/// Rates against `base`: one unit of `base` in each currency, and when
/// they were published (Unix seconds).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Rates {
    pub base: String,
    pub rates: BTreeMap<String, f64>,
    pub updated: i64,
}

/// Whether `code` looks like a currency code.
pub fn valid_currency(code: &str) -> bool {
    code.len() == CURRENCY_CODE_LEN && code.chars().all(|c| c.is_ascii_alphabetic())
}

/// Today's rates against `base` from ExchangeRate-API's open endpoint
/// (free, no key, updated daily).
pub fn exchange_rates(base: &str) -> Result<Rates, String> {
    let base = base.trim().to_ascii_uppercase();
    if !valid_currency(&base) {
        return Err(tf("“{code}” isn't a currency code", &[("code", &base)]).to_string());
    }
    parse_rates(&get(&format!("https://open.er-api.com/v6/latest/{base}"))?)
}

pub fn parse_rates(json: &str) -> Result<Rates, String> {
    #[derive(serde::Deserialize)]
    struct Reply {
        result: String,
        base_code: String,
        rates: BTreeMap<String, f64>,
        time_last_update_unix: i64,
    }
    let r: Reply = serde_json::from_str(json).map_err(|e| e.to_string())?;
    if r.result != "success" {
        return Err(r.result);
    }
    Ok(Rates { base: r.base_code, rates: r.rates, updated: r.time_last_update_unix })
}

// ── Steam ────────────────────────────────────────────────────────────────

/// An installed Steam game.
#[derive(Clone, Debug, PartialEq)]
pub struct SteamGame {
    pub appid: u32,
    pub name: String,
    /// Unix seconds (0 = never).
    pub last_played: i64,
    /// Its library cover (portrait), when Steam cached one.
    pub cover: Option<PathBuf>,
}

/// `"key"  "value"` pairs of a Valve KeyValues text (nesting flattened).
fn kv_pairs(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split('"').collect();
        // `"key"  "value"` splits into ["", key, ws, value, ""].
        if parts.len() >= 5 {
            out.push((parts[1].to_string(), parts[3].to_string()));
        }
    }
    out
}

/// Steam's own folder (the native install, else the Flatpak).
fn steam_root() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    [home.join(".local/share/Steam"), home.join(".steam/steam"), home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam")].into_iter().find(|p| p.join("steamapps").is_dir())
}

/// Tools, runtimes and redistributables, not games.
fn is_tool(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["steam linux runtime", "proton", "steamworks common", "steamvr"].iter().any(|t| n.starts_with(t))
}

/// Installed games, most recently played first.
pub fn steam_games() -> Vec<SteamGame> {
    let Some(root) = steam_root() else { return Vec::new() };
    let libraries = std::fs::read_to_string(root.join("steamapps/libraryfolders.vdf")).map(|t| kv_pairs(&t).into_iter().filter(|(k, _)| k == "path").map(|(_, v)| PathBuf::from(v)).collect::<Vec<_>>()).unwrap_or_default();
    let mut seen = HashSet::new();
    let mut games = Vec::new();
    for lib in std::iter::once(root.clone()).chain(libraries) {
        let Ok(dir) = std::fs::read_dir(lib.join("steamapps")) else { continue };
        for e in dir.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !(name.starts_with("appmanifest_") && name.ends_with(".acf")) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
            if let Some(g) = parse_manifest(&text, &root)
                && seen.insert(g.appid)
                && !is_tool(&g.name)
            {
                games.push(g);
            }
        }
    }
    games.sort_by(|a, b| b.last_played.cmp(&a.last_played).then(a.name.cmp(&b.name)));
    games
}

/// A game from its `appmanifest_*.acf`, its cover under `root`.
pub fn parse_manifest(text: &str, root: &Path) -> Option<SteamGame> {
    let kv = kv_pairs(text);
    let get = |k: &str| kv.iter().find(|(key, _)| key.eq_ignore_ascii_case(k)).map(|(_, v)| v.clone());
    let appid: u32 = get("appid")?.parse().ok()?;
    let name = get("name")?;
    let last_played = get("LastPlayed").and_then(|v| v.parse().ok()).unwrap_or(0);
    let cache = root.join("appcache/librarycache");
    let cover = [cache.join(appid.to_string()).join("library_600x900.jpg"), cache.join(format!("{appid}_library_600x900.jpg"))].into_iter().find(|p| p.is_file());
    Some(SteamGame { appid, name, last_played, cover })
}

// ── Claude Code usage ────────────────────────────────────────────────────

/// Tokens of one day, as Claude Code logged them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DayTokens {
    pub input: u64,
    pub output: u64,
    /// Cache writes and reads.
    pub cache: u64,
}

impl DayTokens {
    pub fn total(&self) -> u64 {
        self.input + self.output + self.cache
    }
}

/// Per-file results kept between reads: (length, modified) → its days.
#[derive(Default)]
pub struct UsageCache {
    files: HashMap<PathBuf, (u64, SystemTime, BTreeMap<NaiveDate, DayTokens>)>,
}

/// Where Claude Code keeps its session logs.
pub fn claude_logs() -> Option<PathBuf> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".claude")))?.join("projects");
    dir.is_dir().then_some(dir)
}

/// Tokens per local day of the last `days` days from the session logs
/// changed in that time (files read again only when they changed).
pub fn claude_usage(days: u32, cache: &mut UsageCache) -> BTreeMap<NaiveDate, DayTokens> {
    let mut out = BTreeMap::new();
    let Some(root) = claude_logs() else { return out };
    let since = SystemTime::now() - Duration::from_secs(u64::from(days) * SECONDS_PER_DAY);
    let first = Local::now().date_naive() - chrono::Duration::days(i64::from(days.saturating_sub(1)));
    let mut keep = HashSet::new();
    let projects = std::fs::read_dir(&root).map(|d| d.flatten().map(|e| e.path()).collect::<Vec<_>>()).unwrap_or_default();
    for project in projects {
        let Ok(files) = std::fs::read_dir(&project) else { continue };
        for f in files.flatten() {
            let path = f.path();
            let Ok(meta) = f.metadata() else { continue };
            let Ok(modified) = meta.modified() else { continue };
            if path.extension().is_none_or(|e| e != "jsonl") || modified < since {
                continue;
            }
            keep.insert(path.clone());
            let fresh = cache.files.get(&path).is_some_and(|(len, at, _)| *len == meta.len() && *at == modified);
            if !fresh {
                let per_day = std::fs::read_to_string(&path).map(|t| parse_usage(&t)).unwrap_or_default();
                cache.files.insert(path.clone(), (meta.len(), modified, per_day));
            }
            if let Some((_, _, per_day)) = cache.files.get(&path) {
                for (d, t) in per_day.range(first..) {
                    let e: &mut DayTokens = out.entry(*d).or_default();
                    e.input += t.input;
                    e.output += t.output;
                    e.cache += t.cache;
                }
            }
        }
    }
    cache.files.retain(|p, _| keep.contains(p));
    out
}

const SECONDS_PER_DAY: u64 = 86_400;

/// Tokens per local day of one session log. A message's usage is logged
/// once per streamed chunk: each (message, request) counts once.
pub fn parse_usage(text: &str) -> BTreeMap<NaiveDate, DayTokens> {
    #[derive(serde::Deserialize)]
    struct Usage {
        #[serde(default)]
        input_tokens: u64,
        #[serde(default)]
        output_tokens: u64,
        #[serde(default)]
        cache_creation_input_tokens: u64,
        #[serde(default)]
        cache_read_input_tokens: u64,
    }
    #[derive(serde::Deserialize)]
    struct Message {
        id: Option<String>,
        usage: Option<Usage>,
    }
    #[derive(serde::Deserialize)]
    struct Line {
        #[serde(rename = "type")]
        kind: Option<String>,
        timestamp: Option<String>,
        #[serde(rename = "requestId")]
        request: Option<String>,
        message: Option<Message>,
    }
    let mut seen = HashSet::new();
    let mut out: BTreeMap<NaiveDate, DayTokens> = BTreeMap::new();
    for raw in text.lines() {
        if !raw.contains("\"usage\"") {
            continue;
        }
        let Ok(line) = serde_json::from_str::<Line>(raw) else { continue };
        if line.kind.as_deref() != Some("assistant") {
            continue;
        }
        let (Some(msg), Some(ts)) = (line.message, line.timestamp) else { continue };
        let Some(u) = msg.usage else { continue };
        if let Some(id) = &msg.id
            && !seen.insert((id.clone(), line.request.clone().unwrap_or_default()))
        {
            continue;
        }
        let Ok(at) = DateTime::parse_from_rfc3339(&ts) else { continue };
        let e = out.entry(at.with_timezone(&Local).date_naive()).or_default();
        e.input += u.input_tokens;
        e.output += u.output_tokens;
        e.cache += u.cache_creation_input_tokens + u.cache_read_input_tokens;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contribution_calendar_markup_reads() {
        let html = r#"<table><tr><td tabindex="0" data-ix="0" aria-selected="false" aria-describedby="contribution-graph-legend-level-0" style="width: 10px" data-date="2026-01-04" id="contribution-day-component-0-0" data-level="0" role="gridcell" class="ContributionCalendar-day"></td>
<td data-date="2026-01-05" id="contribution-day-component-1-0" data-level="3" class="ContributionCalendar-day"></td></tr></table>
<tool-tip id="tooltip-1" for="contribution-day-component-0-0" popover="manual">No contributions on January 4th.</tool-tip>
<tool-tip id="tooltip-2" for="contribution-day-component-1-0" popover="manual">7 contributions on January 5th.</tool-tip>"#;
        let days = parse_contributions(html);
        assert_eq!(days.len(), 2);
        assert_eq!((days[0].level, days[0].count), (0, 0));
        assert_eq!((days[1].date.to_string().as_str(), days[1].level, days[1].count), ("2026-01-05", 3, 7));
        assert!(valid_github_user("octo-cat") && !valid_github_user("a/../b") && !valid_github_user("-x") && !valid_github_user(""));
    }

    #[test]
    fn rates_parse_and_codes_are_checked() {
        let r = parse_rates(r#"{"result":"success","base_code":"USD","time_last_update_unix":1700000000,"rates":{"USD":1,"EGP":48.7,"EUR":0.92}}"#).unwrap();
        assert_eq!(r.rates.get("EGP"), Some(&48.7));
        assert!(parse_rates(r#"{"result":"error","base_code":"X","time_last_update_unix":0,"rates":{}}"#).is_err());
        assert!(valid_currency("egp") && !valid_currency("EURO") && !valid_currency("U$D"));
    }

    #[test]
    fn steam_manifests_read() {
        let acf = "\"AppState\"\n{\n\t\"appid\"\t\t\"570\"\n\t\"name\"\t\t\"Dota 2\"\n\t\"LastPlayed\"\t\t\"1700000000\"\n}\n";
        let g = parse_manifest(acf, Path::new("/nonexistent")).unwrap();
        assert_eq!((g.appid, g.name.as_str(), g.last_played, g.cover), (570, "Dota 2", 1_700_000_000, None));
        assert!(is_tool("Proton 9.0") && !is_tool("Portal 2"));
    }

    #[test]
    fn usage_counts_each_message_once() {
        let line = |id: &str, req: &str, ts: &str| format!(r#"{{"type":"assistant","timestamp":"{ts}","requestId":"{req}","message":{{"id":"{id}","usage":{{"input_tokens":2,"cache_creation_input_tokens":100,"cache_read_input_tokens":50,"output_tokens":10}}}}}}"#);
        let text = [line("m1", "r1", "2026-03-01T10:00:00Z"), line("m1", "r1", "2026-03-01T10:00:01Z"), line("m2", "r2", "2026-03-01T11:00:00Z"), r#"{"type":"user","message":{"content":"hi"}}"#.into()].join("\n");
        let days = parse_usage(&text);
        let total: u64 = days.values().map(DayTokens::total).sum();
        assert_eq!(total, 2 * (2 + 10 + 150));
    }
}
