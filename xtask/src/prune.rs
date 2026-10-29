//! `cargo xtask prune [--max-gib N] [--watch MINUTES] [--dir PATH]…
//! [--cargo-cache] [--dry-run]`: keeps Mandara's build artefacts under a size
//! limit.
//!
//! It measures every target directory of the project (`target/`, the
//! example plugins' and tools' `target/`, `$CARGO_TARGET_DIR` and each
//! `--dir`). Over the limit, it deletes in this order, stopping once the
//! total is back under [`LOW_WATER`] of the limit:
//!
//! 1. the `incremental/` caches of debug-like profiles,
//! 2. whole debug-like profile directories (`debug`, `test`, any custom dev
//!    profile, the target directories other tools and agents left inside
//!    `target/`),
//! 3. the `incremental/` caches of the other profiles,
//! 4. whole optimized profile directories other than `release`,
//! 5. `release` itself,
//! 6. with `--cargo-cache`, the unpacked crate sources in
//!    `$CARGO_HOME/registry/src` (cargo unpacks them again from its download
//!    cache when needed), only while no cargo or rustc is running.
//!
//! A profile directory whose `.cargo-lock` is held by a running build is
//! skipped, and the lock is held while it is deleted so a build can't start
//! in the middle. `target/sandbox` (the demo desktop's home) counts toward
//! the size but is never deleted. `--watch` repeats the check every
//! MINUTES; run it from a binary outside the directories it prunes.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::{Result, flag, opt, root};

/// Default size limit of all build artefacts together, in GiB.
const DEFAULT_MAX_GIB: f64 = 50.0;
/// Pruning stops once the artefacts are back under this fraction of the
/// limit, so a single build doesn't push them straight over it again.
const LOW_WATER: f64 = 0.6;
/// Bytes per GiB.
const GIB: f64 = (1u64 << 30) as f64;
/// `st_blocks` counts 512-byte units.
const BLOCK_BYTES: u64 = 512;
/// Seconds per minute (`--watch`).
const SECS_PER_MINUTE: u64 = 60;
/// Cargo's lock file in each profile directory.
const CARGO_LOCK: &str = ".cargo-lock";
/// Cargo marks the root of every target directory with this file.
const CACHEDIR_TAG: &str = "CACHEDIR.TAG";
/// Incremental compilation caches inside a profile directory.
const INCREMENTAL: &str = "incremental";
/// The release profile, pruned last.
const RELEASE: &str = "release";
/// Optimized profiles: kept until the debug ones are gone.
const OPTIMIZED_PROFILES: &[&str] = &[RELEASE, "release-native", "profiling"];
/// Never deleted: the demo sandbox's home and generated data.
const PROTECTED: &[&str] = &["sandbox"];
/// How deep under a target root profile directories are looked for
/// (`target/<profile>`, `target/<triple>/<profile>`, `target/<tool>/<profile>`,
/// `target/<tool>/<triple>/<profile>`).
const PROFILE_SEARCH_DEPTH: usize = 3;
/// Process names that mean a build is running (`--cargo-cache`).
const BUILD_PROCESSES: &[&str] = &["cargo", "rustc"];

/// What one pruning step deletes.
#[derive(Clone, Debug, PartialEq)]
struct Candidate {
    path: PathBuf,
    /// The profile directory whose lock guards it (`None` for cargo's cache).
    lock_dir: Option<PathBuf>,
}

pub fn prune(args: &[String]) -> Result<()> {
    let max_gib = match opt(args, "--max-gib") {
        Some(v) => v.parse::<f64>().map_err(|_| format!("--max-gib: not a number: {v}"))?,
        None => DEFAULT_MAX_GIB,
    };
    let watch = opt(args, "--watch").map(|v| v.parse::<u64>().map_err(|_| format!("--watch: not a number of minutes: {v}"))).transpose()?;
    let dry_run = flag(args, "--dry-run");
    let cargo_cache = flag(args, "--cargo-cache");
    let roots = target_roots(args);
    loop {
        prune_once(&roots, max_gib, cargo_cache, dry_run)?;
        let Some(minutes) = watch else { return Ok(()) };
        std::thread::sleep(Duration::from_secs(minutes * SECS_PER_MINUTE));
    }
}

/// The project's target directories (existing ones only).
fn target_roots(args: &[String]) -> Vec<PathBuf> {
    let repo = root();
    let mut roots = vec![repo.join("target")];
    for group in ["plugins/examples", "tools"] {
        if let Ok(entries) = std::fs::read_dir(repo.join(group)) {
            roots.extend(entries.flatten().map(|e| e.path().join("target")));
        }
    }
    roots.extend(std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from));
    roots.extend(args.windows(2).filter(|w| w[0] == "--dir").map(|w| PathBuf::from(&w[1])));
    roots.retain(|r| r.is_dir());
    roots.sort();
    roots.dedup();
    roots
}

fn prune_once(roots: &[PathBuf], max_gib: f64, cargo_cache: bool, dry_run: bool) -> Result<()> {
    let limit = (max_gib * GIB) as u64;
    let mut total: u64 = roots.iter().map(|r| disk_usage(r)).sum();
    println!("build artefacts: {} in {} dirs (limit {})", gib(total), roots.len(), gib(limit));
    if total <= limit {
        return Ok(());
    }
    let low_water = (limit as f64 * LOW_WATER) as u64;
    let mut candidates = plan(roots);
    if cargo_cache {
        candidates.extend(cargo_registry_src().into_iter().map(|path| Candidate { path, lock_dir: None }));
    }
    // What a dry run has "deleted" so far, so a profile isn't counted again
    // after its incremental cache.
    let mut dry_deleted: Vec<(PathBuf, u64)> = Vec::new();
    for c in candidates {
        if total <= low_water {
            break;
        }
        if !c.path.exists() {
            continue;
        }
        if c.lock_dir.is_none() && builds_running() {
            println!("skip {} (a build is running)", c.path.display());
            continue;
        }
        let size = disk_usage(&c.path);
        if dry_run {
            let counted: u64 = dry_deleted.iter().filter(|(p, _)| p.starts_with(&c.path)).map(|(_, s)| s).sum();
            let size = size.saturating_sub(counted);
            println!("would delete {} ({})", c.path.display(), gib(size));
            total = total.saturating_sub(size);
            dry_deleted.push((c.path, size));
            continue;
        }
        // Held for the whole deletion: cargo blocks on it instead of
        // building into a half-deleted directory.
        let _guard = match &c.lock_dir {
            Some(dir) => match try_lock(dir) {
                Some(guard) => Some(guard),
                None => {
                    println!("skip {} (a build is using it)", c.path.display());
                    continue;
                }
            },
            None => None,
        };
        match std::fs::remove_dir_all(&c.path) {
            Ok(()) => {
                println!("deleted {} ({})", c.path.display(), gib(size));
                total = total.saturating_sub(size);
            }
            Err(e) => println!("could not delete {}: {e}", c.path.display()),
        }
    }
    println!("build artefacts now: {}", gib(total));
    Ok(())
}

/// Deletion candidates of every root, in pruning order.
fn plan(roots: &[PathBuf]) -> Vec<Candidate> {
    let mut profiles: Vec<PathBuf> = Vec::new();
    for r in roots {
        find_profiles(r, r, 0, &mut profiles);
    }
    profiles.sort();
    let is_debug = |p: &Path| !OPTIMIZED_PROFILES.contains(&name(p).as_str());
    let incremental = |p: &PathBuf| Candidate { path: p.join(INCREMENTAL), lock_dir: Some(p.clone()) };
    let whole = |p: &PathBuf| Candidate { path: p.clone(), lock_dir: Some(p.clone()) };
    let mut out: Vec<Candidate> = Vec::new();
    out.extend(profiles.iter().filter(|p| is_debug(p)).map(incremental));
    out.extend(profiles.iter().filter(|p| is_debug(p)).map(whole));
    out.extend(profiles.iter().filter(|p| !is_debug(p)).map(incremental));
    out.extend(profiles.iter().filter(|p| !is_debug(p) && name(p) != RELEASE).map(whole));
    out.extend(profiles.iter().filter(|p| name(p) == RELEASE).map(whole));
    out
}

/// Collects the profile directories (those holding a `.cargo-lock`) under
/// `dir`, skipping protected ones.
fn find_profiles(target_root: &Path, dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let Ok(kind) = e.file_type() else { continue };
        if !kind.is_dir() {
            continue;
        }
        let path = e.path();
        if dir == target_root && PROTECTED.contains(&name(&path).as_str()) {
            continue;
        }
        if path.join(CARGO_LOCK).is_file() {
            out.push(path);
        } else if depth + 1 < PROFILE_SEARCH_DEPTH || path.join(CACHEDIR_TAG).is_file() {
            find_profiles(target_root, &path, depth + 1, out);
        }
    }
}

/// Takes the profile's build lock if no cargo holds it.
fn try_lock(profile: &Path) -> Option<File> {
    let lock = File::options().read(true).write(true).open(profile.join(CARGO_LOCK)).ok()?;
    lock.try_lock().ok().map(|()| lock)
}

/// The unpacked registry sources (`$CARGO_HOME/registry/src/<index>`).
fn cargo_registry_src() -> Vec<PathBuf> {
    let home = std::env::var_os("CARGO_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")));
    let Some(src) = home.map(|h| h.join("registry/src")) else { return Vec::new() };
    std::fs::read_dir(src).map(|es| es.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default()
}

/// Whether any cargo or rustc process is running for this user.
fn builds_running() -> bool {
    let Ok(procs) = std::fs::read_dir("/proc") else { return true };
    procs.flatten().any(|p| std::fs::read_to_string(p.path().join("comm")).is_ok_and(|c| BUILD_PROCESSES.contains(&c.trim())))
}

/// Space a file tree takes on disk (symlinks not followed).
fn disk_usage(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
    let own = meta.blocks() * BLOCK_BYTES;
    if !meta.is_dir() {
        return own;
    }
    let Ok(entries) = std::fs::read_dir(path) else { return own };
    own + entries.flatten().map(|e| disk_usage(&e.path())).sum::<u64>()
}

fn name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / GIB)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory for one test.
    fn scratch(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mandara-prune-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A profile directory with a lock file and an incremental cache of
    /// `bytes` bytes.
    fn profile(dir: &Path, bytes: usize) -> PathBuf {
        std::fs::create_dir_all(dir.join(INCREMENTAL)).unwrap();
        std::fs::create_dir_all(dir.join("deps")).unwrap();
        std::fs::write(dir.join(CARGO_LOCK), b"").unwrap();
        std::fs::write(dir.join(INCREMENTAL).join("cache"), vec![1u8; bytes]).unwrap();
        std::fs::write(dir.join("deps").join("lib.rlib"), vec![2u8; bytes]).unwrap();
        dir.to_path_buf()
    }

    #[test]
    fn plans_debug_before_release_and_skips_the_sandbox() {
        let t = scratch("plan").join("target");
        let debug = profile(&t.join("debug"), 1);
        let release = profile(&t.join("release"), 1);
        let native = profile(&t.join("release-native"), 1);
        let cross = profile(&t.join("x86_64-unknown-linux-gnu/debug"), 1);
        std::fs::create_dir_all(t.join("agent")).unwrap();
        std::fs::write(t.join("agent").join(CACHEDIR_TAG), b"").unwrap();
        let agent = profile(&t.join("agent/debug"), 1);
        profile(&t.join("sandbox/debug"), 1);
        let order: Vec<PathBuf> = plan(std::slice::from_ref(&t)).into_iter().map(|c| c.path).collect();
        let pos = |p: &Path| order.iter().position(|o| o == p).unwrap_or_else(|| panic!("{} not planned", p.display()));
        for d in [&debug, &cross, &agent] {
            assert!(pos(&d.join(INCREMENTAL)) < pos(d), "incremental first");
            assert!(pos(d) < pos(&release.join(INCREMENTAL)), "debug before optimized");
        }
        assert!(pos(&release.join(INCREMENTAL)) < pos(&native));
        assert!(pos(&native) < pos(&release), "release last");
        assert_eq!(order.last(), Some(&release));
        assert!(!order.iter().any(|p| p.starts_with(t.join("sandbox"))), "sandbox is protected");
    }

    #[test]
    fn prunes_down_to_the_low_water_mark() {
        let t = scratch("low-water").join("target");
        let debug = profile(&t.join("debug"), 400_000);
        let release = profile(&t.join("release"), 400_000);
        let total = disk_usage(&t);
        // A limit just under the total: dropping debug's incremental cache
        // alone isn't enough to reach 60 %, the whole debug profile is.
        let max_gib = (total - 1) as f64 / GIB;
        prune_once(std::slice::from_ref(&t), max_gib, false, false).unwrap();
        assert!(!debug.exists(), "debug deleted");
        assert!(release.join("deps/lib.rlib").exists(), "release kept");
    }

    #[test]
    fn under_the_limit_nothing_is_deleted() {
        let t = scratch("under").join("target");
        let debug = profile(&t.join("debug"), 1000);
        prune_once(std::slice::from_ref(&t), DEFAULT_MAX_GIB, false, false).unwrap();
        assert!(debug.join(INCREMENTAL).exists());
    }

    #[test]
    fn a_locked_profile_is_left_alone() {
        let t = scratch("locked").join("target");
        let debug = profile(&t.join("debug"), 400_000);
        let held = try_lock(&debug).expect("lock");
        prune_once(std::slice::from_ref(&t), 0.0, false, false).unwrap();
        assert!(debug.join(INCREMENTAL).exists(), "a running build's profile survives");
        drop(held);
        prune_once(std::slice::from_ref(&t), 0.0, false, false).unwrap();
        assert!(!debug.exists());
    }

    #[test]
    fn dry_run_deletes_nothing() {
        let t = scratch("dry").join("target");
        let debug = profile(&t.join("debug"), 400_000);
        prune_once(std::slice::from_ref(&t), 0.0, false, true).unwrap();
        assert!(debug.join(INCREMENTAL).exists());
    }

    #[test]
    fn measures_disk_usage_without_following_symlinks() {
        let t = scratch("du");
        std::fs::write(t.join("a"), vec![0u8; 100_000]).unwrap();
        let before = disk_usage(&t);
        std::os::unix::fs::symlink("/usr", t.join("link")).unwrap();
        assert!(disk_usage(&t) < before + BLOCK_BYTES * 8, "symlink target not counted");
        assert!(before >= 100_000);
    }
}
