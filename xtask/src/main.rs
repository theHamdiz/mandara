//! Mandara's release tooling (`cargo xtask <command>`):
//!
//! - `status [--json]`: every component with its version and whether its
//!   content changed since that version was recorded.
//! - `bump [--dry-run] [--commit] [--tag] [--init]`: gives every changed
//!   component a new semantic version (Conventional Commits since its last
//!   tag decide patch / minor / major), records the new hashes in
//!   `versions.toml`, refreshes `Cargo.lock`, and optionally commits and tags
//!   (`v<shell version>` for the shell, `<component>-v<version>` for each).
//!   With `$GITHUB_OUTPUT` set it reports `released`, `version` and `tag`.
//! - `verify`: fails if any component changed without a bump.
//! - `dist --target <triple> [--bin <path>] [--out dist]`: the release
//!   tarball (binary — its own installer —, desktop entry, icon, docs) and
//!   its SHA-256.
//! - `plugins [--out dist]`: builds every plugin for wasm32-wasip2 and
//!   packages `<id>.mandara.tar.gz` (marketplace format).
//! - `checksums [--out dist]`: `SHA256SUMS` for everything in `dist`.
//! - `build [--target <triple>] [--variant NAME] [--profile P] [--features F]
//!   [--no-default-features] [--out dist] [--no-readme]`: a fingerprinted
//!   release build — source fingerprint, binary SHA-256s and lines of code
//!   in `<out>/mandara-<v>-<target>.build.toml` and the README's build-info
//!   block (see [`buildinfo`]).
//! - `art [--check | --compose [--only ID] [--out DIR]]`: plugin artwork —
//!   builds the icons and covers embedded in the binaries, re-composes the
//!   marketplace covers, or validates every artwork deliverable (see
//!   [`art`]).
//! - `delta`, `apply-delta`, `update-part`, `plugin-delta`: delta updates —
//!   binary patches and the release's `update.json` (see [`delta`]).
//! - `resource-check <BIN> [config.toml]`: the idle resource budget of a
//!   binary in a throw-away headless session (see [`resource_check`]).
//! - `sandbox <command>`: a private headless demo desktop for screenshots
//!   and end-to-end tests (see [`sandbox`]).
//! - `prune [--max-gib N] [--watch MINUTES] [--dir PATH]… [--cargo-cache]
//!   [--dry-run]`: keeps the build artefacts under a size limit, debug
//!   first (see [`prune`]).

mod art;
mod buildinfo;
mod delta;
mod fingerprint;
mod headless;
mod loc;
mod proc;
mod prune;
mod resource_check;
mod sandbox;

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use mandara_version::manifest::{self, Bump, Component, Recorded};
use sha2::{Digest, Sha256};

type Result<T> = std::result::Result<T, String>;

/// Mode of executables in release archives.
const MODE_EXECUTABLE: u32 = 0o755;
/// Mode of data files in release archives.
const MODE_FILE: u32 = 0o644;
/// Exit code for a usage error.
const EXIT_USAGE: i32 = 2;
/// Exit code for a failed command.
const EXIT_FAILURE: i32 = 1;

fn root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.parent().unwrap_or(manifest_dir).to_path_buf()
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).current_dir(root()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn run(cmd: &mut Command) -> Result<()> {
    let status = cmd.current_dir(root()).status().map_err(|e| format!("{cmd:?}: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{cmd:?} failed ({status})")) }
}

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn opt(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn tag_for(c: &Component, version: &str) -> String {
    if c.name == "mandara-app" { format!("v{version}") } else { format!("{}-v{version}", c.name) }
}

struct State {
    components: Vec<Component>,
    hashes: BTreeMap<String, String>,
    record: BTreeMap<String, Recorded>,
}

fn state() -> State {
    let root = root();
    let components = manifest::discover(&root);
    let hashes = manifest::hashes(&root, &components);
    let record = manifest::read_record(&root);
    State { components, hashes, record }
}

impl State {
    fn changed(&self, c: &Component) -> bool {
        self.record.get(&c.name).is_none_or(|r| r.hash != self.hashes[&c.name] || r.version != c.version)
    }

    /// The bump Conventional Commits since the component's last tag ask for.
    fn bump_level(&self, c: &Component) -> Bump {
        let tag = tag_for(c, &c.version);
        let range = if git(&["rev-parse", "--verify", "--quiet", &format!("refs/tags/{tag}")]).is_some() { format!("{tag}..HEAD") } else { "HEAD".to_string() };
        let dir = c.dir.to_string_lossy().to_string();
        let log = git(&["log", "--format=%B%x1e", &range, "--", &dir]).unwrap_or_default();
        let messages: Vec<String> = log.split('\u{1e}').map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).collect();
        manifest::bump_from_commits(&messages)
    }

    fn kinds(&self) -> BTreeMap<String, String> {
        self.components.iter().map(|c| (c.name.clone(), c.kind.as_str().to_string())).collect()
    }
}

fn status(args: &[String]) -> Result<()> {
    let s = state();
    if flag(args, "--json") {
        let rows: Vec<String> = s
            .components
            .iter()
            .map(|c| {
                format!(
                    "{{\"name\":{:?},\"kind\":{:?},\"version\":{:?},\"hash\":{:?},\"changed\":{}}}",
                    c.name,
                    c.kind.as_str(),
                    c.version,
                    s.hashes[&c.name],
                    s.changed(c)
                )
            })
            .collect();
        println!("[{}]", rows.join(","));
        return Ok(());
    }
    println!("{:<26} {:<8} {:<10} {:<34} state", "component", "kind", "version", "content hash");
    for c in &s.components {
        let state = if s.changed(c) { format!("changed → {:?}", s.bump_level(c)).to_lowercase() } else { "released".into() };
        println!("{:<26} {:<8} {:<10} {:<34} {state}", c.name, c.kind.as_str(), c.version, s.hashes[&c.name]);
    }
    Ok(())
}

/// Updates the workspace members' entries in a lock file (offline first),
/// for the root workspace or the crate at `manifest`.
fn refresh_lock(manifest: Option<&std::path::Path>) -> Result<()> {
    let update = |offline: bool| {
        let mut cmd = Command::new("cargo");
        cmd.args(["update", "--workspace", "--quiet"]);
        if offline {
            cmd.arg("--offline");
        }
        if let Some(m) = manifest {
            cmd.arg("--manifest-path").arg(m);
        }
        run(&mut cmd)
    };
    update(true).or_else(|_| update(false)).map_err(|e| format!("refreshing {}: {e}", manifest.map_or_else(|| "Cargo.lock".to_string(), |m| m.display().to_string())).into())
}

/// The Cargo manifests among `manifests` (relative to `root`) whose crate
/// keeps its own lock file outside the root workspace. A plugin's manifests
/// also include its `plugin.toml`, which cargo can't update.
fn own_lock_manifests(root: &Path, manifests: &[PathBuf]) -> Vec<PathBuf> {
    manifests
        .iter()
        .map(|m| root.join(m))
        .filter(|m| m.file_name().is_some_and(|n| n == "Cargo.toml"))
        .filter(|m| m.parent().is_some_and(|d| d != root && d.join("Cargo.lock").exists()))
        .collect()
}

fn bump(args: &[String]) -> Result<()> {
    let dry = flag(args, "--dry-run");
    let init = flag(args, "--init") || !root().join(manifest::RECORD).exists();
    let s = state();
    let mut record = s.record.clone();
    let mut bumped: Vec<(String, String, String)> = Vec::new(); // (name, version, tag)
    for c in &s.components {
        if !s.changed(c) {
            continue;
        }
        let new = if init || !s.record.contains_key(&c.name) { c.version.clone() } else { manifest::bump(&c.version, s.bump_level(c)) };
        if !init && s.record.contains_key(&c.name) {
            println!("{:<26} {} → {new}", c.name, c.version);
        } else {
            println!("{:<26} recorded at {new}", c.name);
        }
        if !dry {
            for m in &c.manifests {
                manifest::set_manifest_version(&root().join(m), &new).map_err(|e| format!("{}: {e}", m.display()))?;
            }
        }
        record.insert(c.name.clone(), Recorded { version: new.clone(), hash: s.hashes[&c.name].clone() });
        if !init && s.record.contains_key(&c.name) {
            bumped.push((c.name.clone(), new.clone(), tag_for(c, &new)));
        }
    }
    if dry {
        println!("(dry run — nothing written)");
        return Ok(());
    }
    // Keep the shell's version as the workspace version too.
    if let Some(app) = record.get("mandara-app") {
        set_workspace_version(&app.version)?;
    }
    if !bumped.is_empty() || init {
        // Refresh Cargo.lock for the new workspace versions (offline).
        refresh_lock(None)?;
        // Crates outside the workspace (example plugins) keep their own
        // lock file; a stale one makes their `--locked` builds fail.
        for c in s.components.iter().filter(|c| bumped.iter().any(|(n, _, _)| *n == c.name)) {
            for manifest in own_lock_manifests(&root(), &c.manifests) {
                refresh_lock(Some(&manifest))?;
            }
        }
        // Version lines are excluded from hashes, so recorded hashes still
        // match; recompute anyway to record the final state exactly.
        let after = state();
        for (name, r) in record.iter_mut() {
            if let Some(h) = after.hashes.get(name) {
                r.hash = h.clone();
            }
        }
        manifest::write_record(&root(), &record, &s.kinds()).map_err(|e| e.to_string())?;
    }
    let app = bumped.iter().find(|(n, _, _)| n == "mandara-app").cloned();
    if bumped.is_empty() {
        println!("{}", if init { "versions.toml initialized" } else { "nothing changed since the last release" });
    }
    if flag(args, "--commit") && (!bumped.is_empty() || init) {
        let summary: Vec<String> = bumped.iter().map(|(n, v, _)| format!("{n} {v}")).collect();
        let title = match &app {
            Some((_, v, _)) => format!("chore(release): mandara v{v}"),
            None if init => "chore(release): record component versions".to_string(),
            None => "chore(release): component updates".to_string(),
        };
        run(Command::new("git").args(["add", "-A", "versions.toml", "Cargo.toml", "Cargo.lock", "crates", "xtask", "plugins", "tools"]))?;
        run(Command::new("git").args(["commit", "--quiet", "-m", &title, "-m", &summary.join("\n")]))?;
        if flag(args, "--tag") {
            for (name, version, tag) in &bumped {
                run(Command::new("git").args(["tag", "-a", tag, "-m", &format!("{name} {version}")]))?;
            }
        }
    }
    if let Ok(path) = std::env::var("GITHUB_OUTPUT") {
        let mut f = std::fs::OpenOptions::new().append(true).create(true).open(path).map_err(|e| e.to_string())?;
        let (released, version, tag) = match &app {
            Some((_, v, t)) => ("true", v.clone(), t.clone()),
            None => ("false", record.get("mandara-app").map(|r| r.version.clone()).unwrap_or_default(), String::new()),
        };
        writeln!(f, "released={released}\nversion={version}\ntag={tag}\nbumped={}", bumped.len()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn set_workspace_version(version: &str) -> Result<()> {
    let path = root().join("Cargo.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut section = String::new();
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim_start().starts_with('[') {
            section = line.trim().to_string();
        }
        if section == "[workspace.package]" && line.trim_start().starts_with("version =") {
            out.push(format!("version = \"{version}\""));
        } else {
            out.push(line.to_string());
        }
    }
    std::fs::write(&path, out.join("\n") + "\n").map_err(|e| e.to_string())
}

fn verify() -> Result<()> {
    let s = state();
    let changed: Vec<&str> = s.components.iter().filter(|c| s.changed(c)).map(|c| c.name.as_str()).collect();
    if changed.is_empty() {
        println!("all {} components match versions.toml", s.components.len());
        Ok(())
    } else {
        Err(format!("changed without a version bump: {} (run `cargo xtask bump`)", changed.join(", ")))
    }
}

/// SHA-256 of a file, streamed (release binaries are ~100 MB).
fn sha256_file(path: &Path) -> Result<String> {
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    let mut file = std::fs::File::open(path).map_err(err)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(err)?;
    Ok(manifest::hex(&hasher.finalize()))
}

/// Reproducible archives: every entry's mtime is `SOURCE_DATE_EPOCH` (0
/// when unset).
fn source_date_epoch() -> u64 {
    std::env::var("SOURCE_DATE_EPOCH").ok().and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// Appends `src` as `dest`, streamed from disk.
fn add_file(tar: &mut tar::Builder<flate2::write::GzEncoder<std::fs::File>>, src: &Path, dest: &str, mode: u32) -> Result<()> {
    let err = |e: std::io::Error| format!("{}: {e}", src.display());
    let file = std::fs::File::open(src).map_err(err)?;
    let mut h = tar::Header::new_gnu();
    h.set_size(file.metadata().map_err(err)?.len());
    h.set_mode(mode);
    h.set_mtime(source_date_epoch());
    h.set_cksum();
    tar.append_data(&mut h, dest, file).map_err(|e| e.to_string())
}

/// `x86_64-unknown-linux-gnu` → `x86_64-linux-gnu`.
fn short_target(target: &str) -> String {
    target.replace("-unknown-", "-")
}

fn dist(args: &[String]) -> Result<()> {
    let target = opt(args, "--target").unwrap_or_else(|| format!("{}-unknown-linux-gnu", std::env::consts::ARCH));
    let out = root().join(opt(args, "--out").unwrap_or_else(|| "dist".into()));
    let bin = opt(args, "--bin").map(PathBuf::from).unwrap_or_else(|| {
        let cross = root().join("target").join(&target).join("release/mandara");
        if cross.exists() { cross } else { root().join("target/release/mandara") }
    });
    if !bin.exists() {
        return Err(format!("{} not found — build first: cargo build --release --locked -p mandara-app --target {target}", bin.display()));
    }
    let version = state().record.get("mandara-app").map(|r| r.version.clone()).unwrap_or_else(|| "0.0.0".into());
    let suffix = opt(args, "--variant").map(|v| format!("-{v}")).unwrap_or_default();
    let name = format!("mandara-{version}-{}{suffix}", short_target(&target));
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let archive = out.join(format!("{name}.tar.gz"));
    let file = std::fs::File::create(&archive).map_err(|e| e.to_string())?;
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(file, flate2::Compression::best()));
    let r = root();
    add_file(&mut tar, &bin, &format!("{name}/bin/mandara"), MODE_EXECUTABLE)?;
    add_file(&mut tar, &r.join("packaging/org.mandara.studio.desktop"), &format!("{name}/share/applications/org.mandara.studio.desktop"), MODE_FILE)?;
    add_file(&mut tar, &r.join("packaging/org.mandara.studio.svg"), &format!("{name}/share/icons/hicolor/scalable/apps/org.mandara.studio.svg"), MODE_FILE)?;
    add_file(&mut tar, &r.join("packaging/pam/mandara-lock"), &format!("{name}/share/pam.d/mandara-lock"), MODE_FILE)?;
    for doc in ["README.md", "PLUGINS.md", "LICENSE.md", "COPYRIGHT.md"] {
        if r.join(doc).exists() {
            add_file(&mut tar, &r.join(doc), &format!("{name}/{doc}"), MODE_FILE)?;
        }
    }
    tar.into_inner().map_err(|e| e.to_string())?.finish().map_err(|e| e.to_string())?;
    let sum = sha256_file(&archive)?;
    std::fs::write(out.join(format!("{name}.tar.gz.sha256")), format!("{sum}  {name}.tar.gz\n")).map_err(|e| e.to_string())?;
    println!("{}", archive.display());
    Ok(())
}

fn plugins(args: &[String]) -> Result<()> {
    let out = root().join(opt(args, "--out").unwrap_or_else(|| "dist".into()));
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let s = state();
    for c in s.components.iter().filter(|c| c.kind == manifest::Kind::Plugin) {
        let dir = root().join(&c.dir);
        if !dir.join("Cargo.toml").exists() {
            continue;
        }
        run(Command::new("cargo").args(["build", "--release", "--locked", "--target", "wasm32-wasip2", "--manifest-path"]).arg(dir.join("Cargo.toml")))?;
        let wasm = std::fs::read_dir(dir.join("target/wasm32-wasip2/release"))
            .map_err(|e| e.to_string())?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "wasm"))
            .ok_or_else(|| format!("{}: no .wasm built", c.name))?;
        let id = c.name.trim_start_matches("plugin-");
        let archive = out.join(format!("{id}.mandara.tar.gz"));
        let file = std::fs::File::create(&archive).map_err(|e| e.to_string())?;
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(file, flate2::Compression::best()));
        add_file(&mut tar, &dir.join("plugin.toml"), &format!("{id}/plugin.toml"), MODE_FILE)?;
        add_file(&mut tar, &wasm, &format!("{id}/plugin.wasm"), MODE_FILE)?;
        for extra in ["icon.png", "cover.png", "README.md"] {
            if dir.join(extra).exists() {
                add_file(&mut tar, &dir.join(extra), &format!("{id}/{extra}"), MODE_FILE)?;
            }
        }
        if let Ok(entries) = std::fs::read_dir(dir.join("locales")) {
            for e in entries.flatten() {
                add_file(&mut tar, &e.path(), &format!("{id}/locales/{}", e.file_name().to_string_lossy()), MODE_FILE)?;
            }
        }
        tar.into_inner().map_err(|e| e.to_string())?.finish().map_err(|e| e.to_string())?;
        println!("{} ({} {})", archive.display(), c.name, c.version);
    }
    Ok(())
}

fn checksums(args: &[String]) -> Result<()> {
    let out = root().join(opt(args, "--out").unwrap_or_else(|| "dist".into()));
    let mut files: Vec<PathBuf> = std::fs::read_dir(&out).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    files.sort();
    let mut lines = String::new();
    for f in files {
        let Some(name) = f.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
        if name == "SHA256SUMS" || name.ends_with(".sha256") {
            continue;
        }
        lines.push_str(&format!("{}  {name}\n", sha256_file(&f)?));
    }
    std::fs::write(out.join("SHA256SUMS"), &lines).map_err(|e| e.to_string())?;
    print!("{lines}");
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    let result = match args.first().map(String::as_str) {
        Some("status") => status(rest),
        Some("bump") => bump(rest),
        Some("verify") => verify(),
        Some("dist") => dist(rest),
        Some("plugins") => plugins(rest),
        Some("checksums") => checksums(rest),
        Some("art") => art::art(rest),
        Some("build") => buildinfo::build(rest),
        Some("delta") => delta::delta(rest),
        Some("apply-delta") => delta::apply_delta(rest),
        Some("update-part") => delta::update_part(rest),
        Some("plugin-delta") => delta::plugin_delta(rest),
        Some("resource-check") => resource_check::resource_check(rest),
        Some("sandbox") => sandbox::sandbox(rest),
        Some("prune") => prune::prune(rest),
        _ => {
            eprintln!("usage: cargo xtask <status [--json] | bump [--dry-run] [--commit] [--tag] [--init] | verify | dist --target <triple> [--bin PATH] [--variant NAME] [--out DIR] | plugins [--out DIR] | checksums [--out DIR] | build [--target <triple>] [--variant NAME] [--profile P] [--features F] [--no-default-features] [--out DIR] [--no-readme] | art [--check | --compose [--only ID] [--out DIR]] | delta --old BIN --new BIN --out PATCH | apply-delta --old BIN --patch PATCH --out BIN [--sha256 HEX] | update-part --target <triple> [--variant NAME] [--out DIR] [--previous OLD.tar.gz] | plugin-delta --new ARCHIVE [--old ARCHIVE] --out DIR | resource-check BIN [config.toml] | sandbox <up | down | shot | matte | msg | pointer | studio | shell | audio | notify | env> … | prune [--max-gib N] [--watch MINUTES] [--dir PATH]… [--cargo-cache] [--dry-run]>");
            std::process::exit(EXIT_USAGE);
        }
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(EXIT_FAILURE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_cargo_manifests_with_their_own_lock_are_refreshed() {
        let root = std::env::temp_dir().join(format!("mandara-xtask-locks-{}", std::process::id()));
        let plugin = root.join("plugins/examples/moon-phase");
        std::fs::create_dir_all(&plugin).unwrap();
        for f in ["Cargo.toml", "plugin.toml", "Cargo.lock"] {
            std::fs::write(plugin.join(f), "").unwrap();
        }
        std::fs::create_dir_all(root.join("crates/app")).unwrap();
        std::fs::write(root.join("crates/app/Cargo.toml"), "").unwrap();
        let manifests: Vec<PathBuf> = ["plugins/examples/moon-phase/plugin.toml", "plugins/examples/moon-phase/Cargo.toml", "crates/app/Cargo.toml"].iter().map(PathBuf::from).collect();
        // The plugin's plugin.toml is skipped (cargo rejects it) and so is a
        // workspace crate without a lock file of its own.
        assert_eq!(own_lock_manifests(&root, &manifests), vec![plugin.join("Cargo.toml")]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
