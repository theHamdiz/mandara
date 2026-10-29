//! Mandara's own updates: **delta first, full download as the fallback**.
//!
//! Every release publishes `update.json` (see `tools/mandara-delta`): per
//! target the full tarball, the `bin/mandara` inside it, and a bsdiff-style
//! patch from the previous release. Updating:
//!
//! 1. **Check** — `releases/latest` of [`REPO`] (or `MANDARA_UPDATE_API`),
//!    then its `update.json` (releases without one fall back to the tarball
//!    naming and `SHA256SUMS`); semver against the running build. Automatic
//!    checks reuse the last answer for the configured interval.
//! 2. **Delta** — only when the installed binary's SHA-256 equals the
//!    published hash of the version it claims to be: download the patch,
//!    verify its hash, rebuild the new binary in memory and verify *its*
//!    hash.
//! 3. **Full** (any delta failure, no patch, or `--full`) — download the
//!    tarball, verify it against `update.json` / `SHA256SUMS`, extract
//!    `bin/mandara`.
//! 4. **Replace** atomically (`mandara.new` → fsync → rename, the previous
//!    binary kept as `mandara.old` for [`install::rollback`]); the caller
//!    restarts the shell.
//!
//! Only installs Mandara made itself are replaced ([`install::Install`]).
//! Marketplace plugins use the same machinery ([`plugins`]).

pub mod fetch;
pub mod install;
pub mod plugins;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mandara_delta::{MANIFEST_FILE, Manifest, SHELL_BINARY};
use mandara_i18n::{t, tf};
use serde::{Deserialize, Serialize};

pub use install::Install;

/// The shell's GitHub repository.
pub const REPO: &str = "theHamdiz/mandara";
/// Environment variable overriding the API base (tests, mirrors).
pub const API_ENV: &str = "MANDARA_UPDATE_API";
/// GitHub's REST API.
pub const DEFAULT_API: &str = "https://api.github.com";
/// Shortest interval between automatic checks.
pub const MIN_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// Largest JSON answer read (release metadata, update.json, SHA256SUMS).
const MAX_JSON_BYTES: u64 = 4 << 20;
/// Largest tarball downloaded.
const MAX_ARCHIVE_BYTES: u64 = 512 << 20;
/// Largest binary extracted or rebuilt.
const MAX_BINARY_BYTES: u64 = 1 << 30;
/// The checksum listing every release carries.
const SUMS_FILE: &str = "SHA256SUMS";
/// The last check's answer, in the cache folder.
const CHECK_CACHE_FILE: &str = "update-check.json";
/// Versions already announced (one notification per version).
const NOTIFIED_FILE: &str = "update-notified.json";
/// Key of the shell in [`NOTIFIED_FILE`].
const SHELL_KEY: &str = "mandara";

/// The API base: `MANDARA_UPDATE_API` or GitHub's.
pub fn api_base() -> String {
    std::env::var(API_ENV).ok().map(|v| v.trim().trim_end_matches('/').to_string()).filter(|v| v.starts_with("https://") || v.starts_with("http://")).unwrap_or_else(|| DEFAULT_API.to_string())
}

/// The running build's version.
pub fn running_version() -> &'static str {
    mandara_version::version()
}

/// The release target this binary was built for: `x86_64-linux-gnu`,
/// `x86_64-linux-gnu-x86-64-v3` (compiled with the v3 features, as the
/// tuned release is), `aarch64-linux-gnu`, or the musl builds
/// `x86_64-linux-musl` / `aarch64-linux-musl`. Updates stay on the libc and
/// variant `mandara install` picked.
pub fn running_target() -> String {
    let libc = if cfg!(target_env = "musl") { "musl" } else { "gnu" };
    // The x86-64-v3 tuning is only released for glibc.
    let v3 = libc == "gnu" && cfg!(all(target_arch = "x86_64", target_feature = "avx2", target_feature = "bmi2", target_feature = "fma", target_feature = "movbe"));
    format!("{}-linux-{libc}{}", std::env::consts::ARCH, if v3 { "-x86-64-v3" } else { "" })
}

/// `latest` is a newer semantic version than `current` (pre-releases sort
/// before their release; anything unparsable is never newer).
pub fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |v: &str| semver::Version::parse(v.trim().trim_start_matches('v')).ok();
    matches!((parse(latest), parse(current)), (Some(l), Some(c)) if l > c)
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default()
}

/// A release asset to download.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Download {
    pub file: String,
    pub url: String,
    pub size: u64,
    /// Published SHA-256 (`None`: look it up in `SHA256SUMS`).
    pub sha256: Option<String>,
}

/// A patch from the installed version.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchDownload {
    pub from: String,
    /// The installed binary must hash to this.
    pub from_sha256: String,
    pub download: Download,
}

/// What a check found.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub target: String,
    pub available: bool,
    /// The release page.
    pub notes_url: String,
    /// The full tarball for this target.
    pub full: Option<Download>,
    /// The patch from `current` (delta update), when published.
    pub patch: Option<PatchDownload>,
    /// The new binary's hash and size (from update.json).
    pub binary_sha256: Option<String>,
    pub binary_size: Option<u64>,
    /// `SHA256SUMS` of the release.
    pub sums_url: Option<String>,
    /// When the check ran (seconds since the epoch).
    pub checked_at: u64,
}

impl UpdateInfo {
    /// Bytes the update downloads: the patch when one applies, else the
    /// tarball.
    pub fn download_size(&self, full_only: bool) -> Option<u64> {
        match (&self.patch, &self.full) {
            (Some(p), _) if !full_only => Some(p.download.size),
            (_, Some(f)) => Some(f.size),
            _ => None,
        }
    }

    pub fn full_size(&self) -> Option<u64> {
        self.full.as_ref().map(|f| f.size)
    }
}

#[derive(Deserialize)]
pub(crate) struct ReleaseJson {
    pub(crate) tag_name: String,
    #[serde(default)]
    pub(crate) html_url: String,
    #[serde(default)]
    pub(crate) assets: Vec<AssetJson>,
}

#[derive(Clone, Deserialize)]
pub(crate) struct AssetJson {
    pub(crate) name: String,
    pub(crate) browser_download_url: String,
    #[serde(default)]
    pub(crate) size: u64,
}

pub(crate) fn asset_url(assets: &[AssetJson], name: &str) -> Option<(String, u64)> {
    assets.iter().find(|a| a.name == name).map(|a| (a.browser_download_url.clone(), a.size))
}

pub(crate) fn unexpected(e: impl std::fmt::Display) -> String {
    tf("Unexpected answer from GitHub: {error}", &[("error", &e)])
}

/// Checks `api` for a release newer than `current` for `target` (no cache).
pub fn check_with(api: &str, current: &str, target: &str) -> Result<UpdateInfo, String> {
    let body = fetch::get(&format!("{api}/repos/{REPO}/releases/latest"), MAX_JSON_BYTES, &|_| {}).map_err(|f| match f {
        fetch::Fetch::NotFound => t("No Mandara release is published yet").to_string(),
        other => other.into(),
    })?;
    let release: ReleaseJson = serde_json::from_slice(&body).map_err(unexpected)?;
    let latest = release.tag_name.trim_start_matches('v').to_string();
    let mut info = UpdateInfo {
        current: current.to_string(),
        available: is_newer(&latest, current),
        latest: latest.clone(),
        target: target.to_string(),
        notes_url: release.html_url.clone(),
        sums_url: asset_url(&release.assets, SUMS_FILE).map(|(u, _)| u),
        checked_at: now_secs(),
        ..Default::default()
    };
    let manifest = match asset_url(&release.assets, MANIFEST_FILE) {
        Some((url, _)) => match fetch::get(&url, MAX_JSON_BYTES, &|_| {}).map_err(String::from).and_then(|b| Manifest::parse(&b)) {
            Ok(m) if m.version.trim_start_matches('v') == latest => Some(m),
            Ok(m) => {
                tracing::warn!("update.json describes {} but the release is {latest}; ignoring it", m.version);
                None
            }
            Err(e) => {
                tracing::warn!("update.json: {e}; falling back to the tarball");
                None
            }
        },
        None => None,
    };
    match manifest.as_ref().and_then(|m| m.targets.get(target)) {
        Some(entry) => {
            info.full = asset_url(&release.assets, &entry.archive.file).map(|(url, _)| Download { file: entry.archive.file.clone(), url, size: entry.archive.size, sha256: Some(entry.archive.sha256.clone()) });
            info.binary_sha256 = Some(entry.binary.sha256.clone());
            info.binary_size = Some(entry.binary.size);
            info.patch = manifest.as_ref().and_then(|m| m.patch_for(target, current)).and_then(|p| {
                asset_url(&release.assets, &p.file).map(|(url, _)| PatchDownload { from: p.from.clone(), from_sha256: p.from_sha256.clone(), download: Download { file: p.file.clone(), url, size: p.size, sha256: Some(p.sha256.clone()) } })
            });
        }
        None => {
            let file = format!("mandara-{latest}-{target}.tar.gz");
            info.full = asset_url(&release.assets, &file).map(|(url, size)| Download { file, url, size, sha256: None });
        }
    }
    Ok(info)
}

fn cache_path() -> PathBuf {
    mandara_config::paths::cache_dir().join(CHECK_CACHE_FILE)
}

/// The last check's answer, if it was made by this build (no network).
pub fn cached() -> Option<UpdateInfo> {
    let info: UpdateInfo = serde_json::from_slice(&std::fs::read(cache_path()).ok()?).ok()?;
    (info.current == running_version() && info.target == running_target()).then_some(info)
}

/// Checks for an update. Without `force`, an answer younger than
/// `interval` is reused (automatic checks never hit GitHub more often).
pub fn check(force: bool, interval: Duration) -> Result<UpdateInfo, String> {
    if !force
        && let Some(info) = cached()
        && now_secs().saturating_sub(info.checked_at) < interval.max(MIN_INTERVAL).as_secs()
    {
        return Ok(info);
    }
    let info = check_with(&api_base(), running_version(), &running_target())?;
    let path = cache_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, serde_json::to_vec_pretty(&info).unwrap_or_default());
    Ok(info)
}

/// Records that `version` of `key` was announced; `true` the first time
/// (so each version is announced once).
pub fn first_announcement(key: &str, version: &str) -> bool {
    let path = mandara_config::paths::cache_dir().join(NOTIFIED_FILE);
    let mut seen: std::collections::BTreeMap<String, String> = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    if seen.get(key).is_some_and(|v| v == version) {
        return false;
    }
    seen.insert(key.to_string(), version.to_string());
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, serde_json::to_vec_pretty(&seen).unwrap_or_default());
    true
}

/// [`first_announcement`] for the shell.
pub fn first_shell_announcement(version: &str) -> bool {
    first_announcement(SHELL_KEY, version)
}

/// How an update was (or would be) installed.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Method {
    /// Binary patch against the installed version.
    Delta,
    /// The full archive.
    Full,
}

/// Update options.
#[derive(Copy, Clone, Debug, Default)]
pub struct ApplyOptions {
    /// Skip the patch and download the full archive.
    pub full_only: bool,
    /// Only report what would be downloaded.
    pub dry_run: bool,
}

/// What an update did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplyReport {
    pub method: Method,
    pub version: String,
    /// Bytes actually downloaded (a failed patch attempt included).
    pub downloaded: u64,
    /// Size of the full archive, for comparison.
    pub full_size: u64,
    /// Why the patch wasn't used, when it fell back to the full archive.
    pub fallback: Option<String>,
    pub dry_run: bool,
}

/// Progress of an update.
#[derive(Clone, Debug, PartialEq)]
pub enum Progress {
    /// A step (translated).
    Step(String),
    /// Bytes downloaded of the current file.
    Bytes { done: u64, total: u64 },
}

/// Where the new binary comes from in [`apply_with`].
pub(crate) struct Plan<'a> {
    pub(crate) patch: Option<&'a PatchDownload>,
    pub(crate) full: Option<&'a Download>,
    pub(crate) binary_sha256: Option<&'a str>,
    pub(crate) binary_size: Option<u64>,
    pub(crate) sums_url: Option<&'a str>,
}

/// Downloads and verifies `d`; errors carry the bytes received.
fn download(d: &Download, limit: u64, progress: &dyn Fn(Progress)) -> Result<Vec<u8>, (String, u64)> {
    let total = d.size;
    let bytes = fetch::get(&d.url, limit, &|done| progress(Progress::Bytes { done, total })).map_err(|e| (String::from(e), 0))?;
    if let Some(want) = &d.sha256 {
        let got = mandara_delta::sha256_hex(&bytes);
        if !got.eq_ignore_ascii_case(want) {
            return Err((tf("{file} failed verification (sha256 {got})", &[("file", &d.file), ("got", &got)]), bytes.len() as u64));
        }
    }
    Ok(bytes)
}

/// The patch path: rebuilds the new binary from `old`. Errors carry the
/// bytes already downloaded.
pub(crate) fn try_delta(plan: &Plan, old: &[u8], progress: &dyn Fn(Progress)) -> Result<(Vec<u8>, u64), (String, u64)> {
    let patch = plan.patch.ok_or_else(|| (t("no patch from your version is published").to_string(), 0))?;
    let (Some(sha), Some(size)) = (plan.binary_sha256, plan.binary_size) else { return Err((t("the release doesn't publish the new binary's checksum").to_string(), 0)) };
    if !mandara_delta::sha256_hex(old).eq_ignore_ascii_case(&patch.from_sha256) {
        return Err((tf("the installed copy isn't the published {version} build", &[("version", &patch.from)]), 0));
    }
    progress(Progress::Step(tf("Downloading the smart update ({size})…", &[("size", &human_size(patch.download.size))])));
    let bytes = download(&patch.download, patch.download.size.max(1), progress)?;
    let spent = bytes.len() as u64;
    progress(Progress::Step(t("Applying the patch…").to_string()));
    let new = mandara_delta::apply_verified(old, &bytes, size, sha).map_err(|e| (e, spent))?;
    Ok((new, spent))
}

/// The full path: downloads and verifies the archive, extracts `binary`.
pub(crate) fn full_archive(plan: &Plan, progress: &dyn Fn(Progress)) -> Result<(Vec<u8>, u64), String> {
    let full = plan.full.ok_or_else(|| t("The release has no download for this system").to_string())?;
    let mut full = full.clone();
    if full.sha256.is_none() {
        let sums = plan.sums_url.map(|u| fetch::get(u, MAX_JSON_BYTES, &|_| {}).map_err(String::from)).transpose()?;
        full.sha256 = sums.and_then(|s| mandara_delta::sha256sums_lookup(&String::from_utf8_lossy(&s), &full.file));
    }
    if full.sha256.is_none() {
        return Err(tf("{file} has no published checksum; not installing it", &[("file", &full.file)]));
    }
    progress(Progress::Step(tf("Downloading the full update ({size})…", &[("size", &human_size(full.size))])));
    let bytes = download(&full, MAX_ARCHIVE_BYTES, progress).map_err(|(e, _)| e)?;
    let spent = bytes.len() as u64;
    Ok((bytes, spent))
}

fn extract_shell(archive: &[u8], plan: &Plan) -> Result<Vec<u8>, String> {
    let bin = mandara_delta::archive::extract_file(archive, SHELL_BINARY, MAX_BINARY_BYTES).map_err(|e| e.to_string())?.ok_or_else(|| tf("The archive has no {path}", &[("path", &SHELL_BINARY)]))?;
    if let Some(sha) = plan.binary_sha256
        && !mandara_delta::sha256_hex(&bin).eq_ignore_ascii_case(sha)
    {
        return Err(tf("{path} in the archive doesn't match update.json", &[("path", &SHELL_BINARY)]));
    }
    if !bin.starts_with(install::ELF_MAGIC) {
        return Err(tf("{path} in the archive isn't an executable", &[("path", &SHELL_BINARY)]));
    }
    Ok(bin)
}

/// Installs `info`'s update over `exe` (an updatable install): patch first,
/// the full archive on any failure. The caller restarts the shell.
pub fn apply(info: &UpdateInfo, exe: &Path, opts: ApplyOptions, progress: &dyn Fn(Progress)) -> Result<ApplyReport, String> {
    if !info.available {
        return Err(tf("Mandara {version} is already the latest version", &[("version", &info.current)]));
    }
    let plan = Plan { patch: info.patch.as_ref(), full: info.full.as_ref(), binary_sha256: info.binary_sha256.as_deref(), binary_size: info.binary_size, sums_url: info.sums_url.as_deref() };
    let full_size = info.full_size().unwrap_or_default();
    let use_patch = !opts.full_only && plan.patch.is_some();
    if opts.dry_run {
        let method = if use_patch { Method::Delta } else { Method::Full };
        let fallback = (!opts.full_only && plan.patch.is_none()).then(|| t("no patch from your version is published").to_string());
        return Ok(ApplyReport { method, version: info.latest.clone(), downloaded: info.download_size(opts.full_only).unwrap_or_default(), full_size, fallback, dry_run: true });
    }
    let mut downloaded = 0;
    let mut fallback = None;
    if !opts.full_only {
        let old = std::fs::read(exe).map_err(|e| format!("{}: {e}", exe.display()))?;
        match try_delta(&plan, &old, progress) {
            Ok((new, spent)) => {
                progress(Progress::Step(t("Installing…").to_string()));
                install::replace(exe, &new).map_err(|e| format!("{}: {e}", exe.display()))?;
                return Ok(ApplyReport { method: Method::Delta, version: info.latest.clone(), downloaded: spent, full_size, fallback: None, dry_run: false });
            }
            Err((reason, spent)) => {
                tracing::info!("delta update unavailable ({reason}); downloading the full update");
                downloaded += spent;
                progress(Progress::Step(tf("Smart update unavailable ({reason}); using the full download", &[("reason", &reason)])));
                fallback = Some(reason);
            }
        }
    }
    let (archive, spent) = full_archive(&plan, progress)?;
    downloaded += spent;
    let bin = extract_shell(&archive, &plan)?;
    progress(Progress::Step(t("Installing…").to_string()));
    install::replace(exe, &bin).map_err(|e| format!("{}: {e}", exe.display()))?;
    Ok(ApplyReport { method: Method::Full, version: info.latest.clone(), downloaded, full_size, fallback, dry_run: false })
}

/// `4.2 MB`, `38 MB`, `512 KB` (decimal units, like download sizes).
pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1000.0;
    const MB: f64 = KB * KB;
    const ONE_DECIMAL_BELOW: f64 = 10.0;
    let b = bytes as f64;
    if b >= MB {
        let mb = b / MB;
        if mb < ONE_DECIMAL_BELOW { format!("{mb:.1} MB") } else { format!("{mb:.0} MB") }
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_ordering() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("v0.1.10", "0.1.9"));
        assert!(is_newer("0.1.2", "0.1.2-dev"), "a release is newer than its dev build");
        assert!(!is_newer("0.1.2-dev", "0.1.2"));
        assert!(!is_newer("0.1.2", "0.1.2"));
        assert!(!is_newer("0.1.1", "0.1.2"));
        assert!(!is_newer("garbage", "0.1.0"));
        assert!(!is_newer("1.0.0", "garbage"));
    }

    #[test]
    fn target_names_match_the_release_tarballs() {
        let t = running_target();
        let libc = if cfg!(target_env = "musl") { "-linux-musl" } else { "-linux-gnu" };
        assert!(t.ends_with(libc) || t.ends_with("-linux-gnu-x86-64-v3"), "{t}");
        assert!(t.starts_with(std::env::consts::ARCH));
    }

    #[test]
    fn sizes_read_like_downloads() {
        assert_eq!(human_size(4_200_000), "4.2 MB");
        assert_eq!(human_size(38_400_000), "38 MB");
        assert_eq!(human_size(48_000), "48 KB");
        assert_eq!(human_size(12), "12 B");
    }

    fn plan_for<'a>(patch: &'a PatchDownload, sha: &'a str, size: u64) -> Plan<'a> {
        Plan { patch: Some(patch), full: None, binary_sha256: Some(sha), binary_size: Some(size), sums_url: None }
    }

    #[test]
    fn delta_is_skipped_when_the_installed_binary_is_not_the_published_one() {
        let patch = PatchDownload { from: "0.1.0".into(), from_sha256: mandara_delta::sha256_hex(b"published"), download: Download { file: "p".into(), url: "http://127.0.0.1:9/never".into(), size: 10, sha256: None } };
        let sha = mandara_delta::sha256_hex(b"new");
        let err = try_delta(&plan_for(&patch, &sha, 3), b"locally modified", &|_| {}).unwrap_err();
        assert_eq!(err.1, 0, "nothing is downloaded for a mismatching binary");
        assert!(err.0.contains("0.1.0"), "{}", err.0);
    }
}
