//! Updates for marketplace plugins, on the shell's machinery: delta first
//! (a patch for `plugin.wasm` plus the small meta archive holding every
//! other file), the full `<id>.mandara.tar.gz` as the fallback.
//!
//! First-party plugins are compiled into the shell and update with it.
//! A plugin release opts in to patches by publishing `update.json`
//! (`mandara-delta plugin`, see PLUGINS.md); without one, updates download
//! the archive (checked against `SHA256SUMS` when the release has it).
//! The plugin folder is swapped atomically (staged next to it, renamed into
//! place) and the previous folder kept for [`rollback`]; the shell's plugin
//! watcher hot-reloads it, and the enabled state (keyed by id) is kept.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use mandara_delta::{MANIFEST_FILE, Manifest, PLUGIN_BINARY, PLUGIN_TARGET};
use mandara_i18n::{t, tf};
use serde::{Deserialize, Serialize};

use super::{ApplyOptions, ApplyReport, Download, Method, PatchDownload, Plan, Progress, fetch, full_archive, is_newer, now_secs, try_delta};
use crate::marketplace::{self, Asset, InstallError, Vetting};

/// Plugins checked per run at most (anonymous GitHub allows 60 requests an
/// hour; each plugin costs one or two).
pub const MAX_CHECKS: usize = 24;
/// The last plugin check, in the cache folder.
const CACHE_FILE: &str = "plugin-updates.json";
/// Previous plugin folders, kept for rollback (beside the plugin folder so
/// the swap is a rename, outside it so the plugin host never loads them).
const ROLLBACK_DIR: &str = "plugin-rollback";
/// Largest JSON read (update.json, SHA256SUMS).
const MAX_JSON_BYTES: u64 = 1 << 20;
/// The checksum listing a release may carry.
const SUMS_FILE: &str = "SHA256SUMS";
/// Prefix of plugin keys in the announcement record.
const NOTIFY_PREFIX: &str = "plugin:";

/// One installed marketplace plugin's update state.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginUpdate {
    /// `owner/repo`.
    pub repo: String,
    pub id: String,
    pub dir: PathBuf,
    /// Installed release tag.
    pub installed: String,
    /// Newest release tag.
    pub latest: String,
    pub available: bool,
    pub notes_url: String,
    pub full: Option<Download>,
    pub patch: Option<PatchDownload>,
    /// Every file but `plugin.wasm`, downloaded with a patch.
    pub meta: Option<Download>,
    pub binary_sha256: Option<String>,
    pub binary_size: Option<u64>,
    pub sums_url: Option<String>,
}

impl PluginUpdate {
    /// Bytes the update downloads (patch + meta, or the archive).
    pub fn download_size(&self, full_only: bool) -> Option<u64> {
        match (&self.patch, &self.meta, &self.full) {
            (Some(p), Some(m), _) if !full_only => Some(p.download.size + m.size),
            (_, _, Some(f)) => Some(f.size),
            _ => None,
        }
    }

    /// `v1.2.0` → `1.2.0`.
    pub fn latest_version(&self) -> &str {
        self.latest.trim_start_matches('v')
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Cache {
    checked_at: u64,
    /// The installed set the answer is for (repo → tag).
    installed: BTreeMap<String, String>,
    updates: Vec<PluginUpdate>,
}

fn cache_path() -> PathBuf {
    mandara_config::paths::cache_dir().join(CACHE_FILE)
}

fn plugin_id(dir: &Path) -> String {
    dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

fn asset<'a>(assets: &'a [Asset], name: &str) -> Option<&'a Asset> {
    assets.iter().find(|a| a.name == name)
}

/// Checks one installed plugin (`repo` at `tag`, in `dir`).
pub fn check_one(repo: &str, dir: &Path, tag: &str) -> Result<PluginUpdate, String> {
    let (release, archive) = marketplace::find_release(repo).map_err(|e| match e {
        InstallError::Failed(e) => e,
        other => other.to_string(),
    })?;
    let id = plugin_id(dir);
    let mut u = PluginUpdate {
        repo: repo.to_string(),
        id: id.clone(),
        dir: dir.to_path_buf(),
        installed: tag.to_string(),
        latest: release.tag_name.clone(),
        notes_url: release.html_url.clone(),
        sums_url: asset(&release.assets, SUMS_FILE).map(|a| a.browser_download_url.clone()),
        full: Some(Download { file: archive.name.clone(), url: archive.browser_download_url.clone(), size: archive.size, sha256: None }),
        ..Default::default()
    };
    // Tags that aren't semver (dates, names) update whenever they differ.
    let semver = |v: &str| semver::Version::parse(v.trim_start_matches('v')).is_ok();
    u.available = if semver(&u.latest) && semver(tag) { is_newer(&u.latest, tag) } else { u.latest != tag };
    let manifest = asset(&release.assets, MANIFEST_FILE).and_then(|a| match fetch::get(&a.browser_download_url, MAX_JSON_BYTES, &|_| {}).map_err(String::from).and_then(|b| Manifest::parse(&b)) {
        Ok(m) if m.name == id => Some(m),
        Ok(m) => {
            tracing::info!("{repo}: update.json is for `{}`, not `{id}`", m.name);
            None
        }
        Err(e) => {
            tracing::warn!("{repo}: {e}; updates use the full archive");
            None
        }
    });
    if let Some(entry) = manifest.as_ref().and_then(|m| m.targets.get(PLUGIN_TARGET)) {
        if entry.archive.file == archive.name {
            u.full = Some(Download { file: archive.name.clone(), url: archive.browser_download_url.clone(), size: entry.archive.size, sha256: Some(entry.archive.sha256.clone()) });
        }
        u.binary_sha256 = Some(entry.binary.sha256.clone());
        u.binary_size = Some(entry.binary.size);
        u.meta = entry.meta.as_ref().and_then(|m| asset(&release.assets, &m.file).map(|a| Download { file: m.file.clone(), url: a.browser_download_url.clone(), size: m.size, sha256: Some(m.sha256.clone()) }));
        u.patch = manifest.as_ref().and_then(|m| m.patch_for(PLUGIN_TARGET, tag)).and_then(|p| {
            asset(&release.assets, &p.file).map(|a| PatchDownload { from: p.from.clone(), from_sha256: p.from_sha256.clone(), download: Download { file: p.file.clone(), url: a.browser_download_url.clone(), size: p.size, sha256: Some(p.sha256.clone()) } })
        });
    }
    Ok(u)
}

/// The last answer (no network), dropping plugins no longer installed or
/// updated since.
pub fn cached() -> Vec<PluginUpdate> {
    let Some(cache) = std::fs::read(cache_path()).ok().and_then(|b| serde_json::from_slice::<Cache>(&b).ok()) else { return Vec::new() };
    let installed = marketplace::installed_all();
    cache.updates.into_iter().filter(|u| installed.get(&u.repo).is_some_and(|(_, tag)| *tag == u.installed)).collect()
}

/// Checks every installed marketplace plugin, one request after another
/// (at most [`MAX_CHECKS`]). Without `force`, an answer younger than
/// `interval` for the same installed set is reused. A rate limit or network
/// error stops the batch.
pub fn check_all(force: bool, interval: Duration) -> Result<Vec<PluginUpdate>, String> {
    let installed: BTreeMap<String, (PathBuf, String)> = marketplace::installed_all().into_iter().collect();
    let fingerprint: BTreeMap<String, String> = installed.iter().map(|(r, (_, t))| (r.clone(), t.clone())).collect();
    if !force
        && let Some(cache) = std::fs::read(cache_path()).ok().and_then(|b| serde_json::from_slice::<Cache>(&b).ok())
        && cache.installed == fingerprint
        && now_secs().saturating_sub(cache.checked_at) < interval.max(super::MIN_INTERVAL).as_secs()
    {
        return Ok(cache.updates);
    }
    crate::offline::check()?;
    let mut updates = Vec::new();
    for (repo, (dir, tag)) in installed.iter().take(MAX_CHECKS) {
        updates.push(check_one(repo, dir, tag)?);
    }
    let cache = Cache { checked_at: now_secs(), installed: fingerprint, updates: updates.clone() };
    let path = cache_path();
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&path, serde_json::to_vec_pretty(&cache).unwrap_or_default());
    Ok(updates)
}

/// [`super::first_announcement`] for a plugin version.
pub fn first_announcement(u: &PluginUpdate) -> bool {
    super::first_announcement(&format!("{NOTIFY_PREFIX}{}", u.id), &u.latest)
}

fn rollback_path(id: &str) -> PathBuf {
    mandara_config::paths::data_dir().join(ROLLBACK_DIR).join(id)
}

fn scratch(id: &str) -> PathBuf {
    mandara_config::paths::cache_dir().join("plugin-update").join(id)
}

/// Swaps the unpacked `root` in for the installed folder, keeping the old
/// one for rollback.
fn swap_in(root: &Path, u: &PluginUpdate) -> Result<(), String> {
    let io = |e: std::io::Error| e.to_string();
    std::fs::write(root.join(".source"), format!("{}\n{}\n", u.repo, u.latest)).map_err(io)?;
    let parent = u.dir.parent().map(Path::to_path_buf).unwrap_or_else(marketplace::install_dir);
    let staging = parent.join(format!(".{}.new", u.id));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::rename(root, &staging).or_else(|_| marketplace::copy_tree(root, &staging)).map_err(io)?;
    let backup = rollback_path(&u.id);
    let _ = std::fs::remove_dir_all(&backup);
    if let Some(d) = backup.parent() {
        std::fs::create_dir_all(d).map_err(io)?;
    }
    std::fs::rename(&u.dir, &backup).map_err(io)?;
    if let Err(e) = std::fs::rename(&staging, &u.dir) {
        let _ = std::fs::rename(&backup, &u.dir);
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e.to_string());
    }
    Ok(())
}

/// Checks the unpacked release with `vetting` before it is swapped in. A
/// release that fails stays out (the installed version keeps running) and
/// is flagged, unless forced.
fn admit(root: &Path, u: &PluginUpdate, vetting: &Vetting, progress: &dyn Fn(Progress)) -> Result<(), String> {
    progress(Progress::Step(t("Checking the plugin…").to_string()));
    vetting.admit(&u.repo, &u.latest, root).map_err(|e| e.to_string())
}

/// Updates one plugin: patch + meta first, the full archive on any failure.
/// The new release goes through `vetting` before it replaces the old one.
pub fn apply(u: &PluginUpdate, opts: ApplyOptions, vetting: &Vetting, progress: &dyn Fn(Progress)) -> Result<ApplyReport, String> {
    if !u.available {
        return Err(tf("{plugin} is up to date", &[("plugin", &u.id)]));
    }
    let plan = Plan { patch: u.patch.as_ref(), full: u.full.as_ref(), binary_sha256: u.binary_sha256.as_deref(), binary_size: u.binary_size, sums_url: u.sums_url.as_deref() };
    let full_size = u.full.as_ref().map(|f| f.size).unwrap_or_default();
    let use_patch = !opts.full_only && u.patch.is_some() && u.meta.is_some();
    if opts.dry_run {
        return Ok(ApplyReport { method: if use_patch { Method::Delta } else { Method::Full }, version: u.latest.clone(), downloaded: u.download_size(opts.full_only).unwrap_or_default(), full_size, fallback: None, dry_run: true });
    }
    let scratch = scratch(&u.id);
    let mut downloaded = 0;
    let mut fallback = None;
    if !opts.full_only {
        match delta_folder(u, &plan, &scratch, progress) {
            Ok((root, spent)) => {
                let swapped = admit(&root, u, vetting, progress).and_then(|()| swap_in(&root, u));
                let _ = std::fs::remove_dir_all(&scratch);
                swapped?;
                return Ok(ApplyReport { method: Method::Delta, version: u.latest.clone(), downloaded: spent, full_size, fallback: None, dry_run: false });
            }
            Err((reason, spent)) => {
                tracing::info!("{}: delta update unavailable ({reason}); downloading the archive", u.id);
                downloaded += spent;
                progress(Progress::Step(tf("Smart update unavailable ({reason}); using the full download", &[("reason", &reason)])));
                fallback = Some(reason);
            }
        }
    }
    let result = (|| {
        let plan = Plan { full: u.full.as_ref(), ..plan };
        let (archive, spent) = full_archive(&plan, progress).or_else(|e| if u.full.as_ref().is_some_and(|f| f.sha256.is_none()) && plan.sums_url.is_none() { unverified_archive(u, progress) } else { Err(e) })?;
        downloaded += spent;
        progress(Progress::Step(t("Installing…").to_string()));
        let (root, id) = marketplace::unpack(&archive, &scratch)?;
        if id != u.id {
            return Err(tf("The new release is plugin {new}, not {old}; reinstall it from the marketplace", &[("new", &id), ("old", &u.id)]));
        }
        if let Some(sha) = &u.binary_sha256 {
            let wasm = std::fs::read(root.join(PLUGIN_BINARY)).unwrap_or_default();
            if !mandara_delta::sha256_hex(&wasm).eq_ignore_ascii_case(sha) {
                return Err(tf("{path} in the archive doesn't match update.json", &[("path", &PLUGIN_BINARY)]));
            }
        }
        admit(&root, u, vetting, progress)?;
        swap_in(&root, u)
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    result?;
    Ok(ApplyReport { method: Method::Full, version: u.latest.clone(), downloaded, full_size, fallback, dry_run: false })
}

/// Releases without update.json or SHA256SUMS: the archive as the
/// marketplace installs it (unpacked with the same safety checks).
fn unverified_archive(u: &PluginUpdate, progress: &dyn Fn(Progress)) -> Result<(Vec<u8>, u64), String> {
    let full = u.full.as_ref().ok_or_else(|| t("The release has no plugin archive").to_string())?;
    progress(Progress::Step(tf("Downloading the full update ({size})…", &[("size", &super::human_size(full.size))])));
    let total = full.size;
    let bytes = fetch::get(&full.url, marketplace::MAX_BYTES, &|done| progress(Progress::Bytes { done, total })).map_err(String::from)?;
    let n = bytes.len() as u64;
    Ok((bytes, n))
}

/// The delta path: patch `plugin.wasm`, unpack the meta archive beside it.
fn delta_folder(u: &PluginUpdate, plan: &Plan, scratch: &Path, progress: &dyn Fn(Progress)) -> Result<(PathBuf, u64), (String, u64)> {
    let meta = u.meta.as_ref().ok_or_else(|| (t("no patch from your version is published").to_string(), 0))?;
    let old = std::fs::read(u.dir.join(PLUGIN_BINARY)).map_err(|e| (e.to_string(), 0))?;
    let (wasm, spent) = try_delta(plan, &old, progress)?;
    let total = meta.size;
    let meta_bytes = fetch::get(&meta.url, meta.size.max(1), &|done| progress(Progress::Bytes { done, total })).map_err(|e| (String::from(e), spent))?;
    let spent = spent + meta_bytes.len() as u64;
    if meta.sha256.as_ref().is_some_and(|want| !mandara_delta::sha256_hex(&meta_bytes).eq_ignore_ascii_case(want)) {
        return Err((tf("{file} failed verification (sha256 {got})", &[("file", &meta.file), ("got", &mandara_delta::sha256_hex(&meta_bytes))]), spent));
    }
    let (root, id) = marketplace::unpack(&meta_bytes, scratch).map_err(|e| (e, spent))?;
    if id != u.id {
        return Err((tf("The new release is plugin {new}, not {old}; reinstall it from the marketplace", &[("new", &id), ("old", &u.id)]), spent));
    }
    std::fs::write(root.join(PLUGIN_BINARY), wasm).map_err(|e| (e.to_string(), spent))?;
    Ok((root, spent))
}

/// Puts a plugin's previous folder back.
pub fn rollback(id: &str) -> Result<(), String> {
    let backup = rollback_path(id);
    if !backup.join("plugin.toml").is_file() {
        return Err(tf("No previous version of {plugin} to roll back to", &[("plugin", &id)]));
    }
    let dest = marketplace::install_dir().join(id);
    let _ = std::fs::remove_dir_all(&dest);
    std::fs::rename(&backup, &dest).map_err(|e| e.to_string())
}

/// Whether a previous version of `id` is kept.
pub fn can_roll_back(id: &str) -> bool {
    rollback_path(id).join("plugin.toml").is_file()
}
