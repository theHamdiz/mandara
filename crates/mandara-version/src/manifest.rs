//! Component discovery, content hashing and the `versions.toml` record —
//! shared by this crate's build script (embedded build info) and `xtask`
//! (automatic version bumps). Only `std`, `sha2` and `toml`.
//!
//! A *component* is every workspace crate (`crates/*`, `xtask`) and every
//! plugin (`plugins/**/plugin.toml`). Its content hash covers its own files
//! (minus `target/` and the `version = …` line of its manifests, so writing
//! a new version doesn't change the hash), the workspace `Cargo.toml` and
//! `Cargo.lock` (toolchain-relevant dependency changes), and — recursively —
//! the hashes of the workspace crates it depends on. A changed hash means
//! "this component's build changed" and earns it a version bump.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Binary,
    Library,
    Plugin,
    Tool,
}

impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::Binary => "binary",
            Kind::Library => "library",
            Kind::Plugin => "plugin",
            Kind::Tool => "tool",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Component {
    pub name: String,
    pub kind: Kind,
    /// Directory relative to the workspace root.
    pub dir: PathBuf,
    /// Manifests whose `version` this tool manages (Cargo.toml and, for
    /// plugins, plugin.toml).
    pub manifests: Vec<PathBuf>,
    /// The version currently written in the primary manifest.
    pub version: String,
    /// Workspace crates this component depends on (by package name).
    pub deps: Vec<String>,
}

/// One entry of `versions.toml`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recorded {
    pub version: String,
    pub hash: String,
}

pub const RECORD: &str = "versions.toml";

/// Environment variable through which `cargo xtask build` hands the build's
/// source fingerprint (64 hex digits) to this crate's build script. Unset
/// for plain `cargo build` / `cargo check`, which then embed no fingerprint
/// (`dev`) and never hash the tree.
pub const FINGERPRINT_ENV: &str = "MANDARA_SOURCE_FINGERPRINT";
/// Hex digits of a full source fingerprint (SHA-256).
pub const FINGERPRINT_HEX_LEN: usize = 64;
/// Hex digits of the short source fingerprint shown in `--version`.
pub const FINGERPRINT_SHORT_LEN: usize = 12;

/// Bytes of SHA-256 kept in a component hash (128 bits, 32 hex digits).
pub const HASH_BYTES: usize = 16;
/// Separator hashed after each file name and each file's content, so file
/// boundaries can't shift.
const SEPARATOR: [u8; 1] = [0];

/// Lowercase hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from(DIGITS[usize::from(b >> 4)]));
        out.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    out
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

fn table(text: &str) -> toml::Table {
    toml::from_str(text).unwrap_or_default()
}

/// `version` of `[package]` (or top level for plugin.toml); workspace
/// inheritance resolves to the workspace version.
fn manifest_version(path: &Path, workspace_version: &str) -> String {
    let t = table(&read(path));
    let pkg = t.get("package").and_then(|p| p.as_table()).unwrap_or(&t);
    match pkg.get("version") {
        Some(toml::Value::String(v)) => v.clone(),
        Some(toml::Value::Table(w)) if w.get("workspace").and_then(|x| x.as_bool()) == Some(true) => workspace_version.to_string(),
        _ => "0.0.0".into(),
    }
}

fn workspace_version(root: &Path) -> String {
    let t = table(&read(&root.join("Cargo.toml")));
    t.get("workspace").and_then(|w| w.get("package")).and_then(|p| p.get("version")).and_then(|v| v.as_str()).unwrap_or("0.0.0").to_string()
}

fn path_deps(manifest: &Path) -> Vec<String> {
    let t = table(&read(manifest));
    let mut out = BTreeSet::new();
    for section in ["dependencies", "build-dependencies"] {
        if let Some(deps) = t.get(section).and_then(|d| d.as_table()) {
            for (name, spec) in deps {
                let internal = name.starts_with("mandara-") && (spec.get("workspace").and_then(|w| w.as_bool()) == Some(true) || spec.get("path").is_some());
                if internal {
                    out.insert(name.clone());
                }
            }
        }
    }
    out.into_iter().collect()
}

/// Folder of standalone tools (outside the workspace) that are components.
const TOOLS_DIR: &str = "tools";

/// Every component in the workspace at `root`, sorted by name.
pub fn discover(root: &Path) -> Vec<Component> {
    let wv = workspace_version(root);
    let mut out = Vec::new();
    let ws = table(&read(&root.join("Cargo.toml")));
    let members: Vec<String> = ws.get("workspace").and_then(|w| w.get("members")).and_then(|m| m.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
    for member in members {
        let dir = PathBuf::from(&member);
        let manifest = root.join(&dir).join("Cargo.toml");
        let t = table(&read(&manifest));
        let Some(name) = t.get("package").and_then(|p| p.get("name")).and_then(|n| n.as_str()).map(str::to_string) else { continue };
        let kind = if name == "xtask" {
            Kind::Tool
        } else if t.get("bin").is_some() || root.join(&dir).join("src/main.rs").exists() {
            Kind::Binary
        } else {
            Kind::Library
        };
        out.push(Component { version: manifest_version(&manifest, &wv), deps: path_deps(&manifest), name, kind, manifests: vec![dir.join("Cargo.toml")], dir });
    }
    // Tools outside the workspace (own lock file) that workspace crates
    // depend on by path: tools/<name>/Cargo.toml.
    if let Ok(entries) = std::fs::read_dir(root.join(TOOLS_DIR)) {
        for e in entries.flatten() {
            let manifest = e.path().join("Cargo.toml");
            let t = table(&read(&manifest));
            let Some(name) = t.get("package").and_then(|p| p.get("name")).and_then(|n| n.as_str()).map(str::to_string) else { continue };
            let dir = PathBuf::from(TOOLS_DIR).join(e.file_name());
            out.push(Component { version: manifest_version(&manifest, &wv), deps: path_deps(&manifest), name, kind: Kind::Tool, manifests: vec![dir.join("Cargo.toml")], dir });
        }
    }
    // Plugins: any plugin.toml under plugins/ (outside build output).
    let mut stack = vec![root.join("plugins")];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() && p.file_name().is_some_and(|n| n != "target" && n != "dist") {
                stack.push(p);
            } else if p.file_name().is_some_and(|n| n == "plugin.toml") {
                let dir = p.parent().unwrap().strip_prefix(root).unwrap().to_path_buf();
                let t = table(&read(&p));
                let Some(id) = t.get("id").and_then(|v| v.as_str()) else { continue };
                let mut manifests = vec![dir.join("plugin.toml")];
                if root.join(&dir).join("Cargo.toml").exists() {
                    manifests.push(dir.join("Cargo.toml"));
                }
                out.push(Component { name: format!("plugin-{id}"), kind: Kind::Plugin, version: manifest_version(&p, &wv), deps: Vec::new(), manifests, dir });
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// A Cargo.lock without the versions of path/workspace packages (blocks
/// with no `source`), which change on every bump.
fn strip_lock_versions(text: &str) -> String {
    text.split("[[package]]")
        .map(|block| if block.contains("\nsource = ") { block.to_string() } else { strip_version(block) })
        .collect::<Vec<_>>()
        .join("[[package]]")
}

/// A manifest's text without its `version = "…"` line(s), so recording a
/// new version doesn't change the content hash.
fn strip_version(text: &str) -> String {
    text.lines().filter(|l| { let t = l.trim_start(); !(t.starts_with("version =") || t.starts_with("version=") || t.starts_with("version.workspace")) }).collect::<Vec<_>>().join("\n")
}

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name == "target" || name == "dist" || name.starts_with('.') && name != ".cargo" {
            continue;
        }
        // The directory entry's type needs no extra `stat`; only symlinks
        // are resolved (to follow linked folders like before).
        let is_dir = match e.file_type() {
            Ok(t) if t.is_symlink() => p.is_dir(),
            Ok(t) => t.is_dir(),
            Err(_) => p.is_dir(),
        };
        if is_dir {
            files(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// Content hashes of every component (hex SHA-256, first 16 bytes).
pub fn hashes(root: &Path, components: &[Component]) -> BTreeMap<String, String> {
    // Workspace-wide inputs that affect every build.
    let mut common = Sha256::new();
    for f in ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", ".cargo/config.toml"] {
        let text = read(&root.join(f));
        common.update(f.as_bytes());
        let normalized = match f {
            "Cargo.toml" => strip_version(&text),
            "Cargo.lock" => strip_lock_versions(&text),
            _ => text,
        };
        common.update(normalized.as_bytes());
    }
    let common = common.finalize();
    // Own-content hashes.
    let mut own: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for c in components {
        let mut list = Vec::new();
        files(&root.join(&c.dir), &mut list);
        list.sort();
        let manifests: BTreeSet<PathBuf> = c.manifests.iter().map(|m| root.join(m)).collect();
        let mut h = Sha256::new();
        h.update(common);
        for f in list {
            let rel = f.strip_prefix(root).unwrap_or(&f);
            h.update(rel.to_string_lossy().as_bytes());
            h.update(SEPARATOR);
            if f.file_name().is_some_and(|n| n == "Cargo.lock") {
                h.update(strip_lock_versions(&read(&f)).as_bytes());
            } else if manifests.contains(&f) {
                h.update(strip_version(&read(&f)).as_bytes());
            } else if let Ok(mut file) = std::fs::File::open(&f) {
                // Streamed: assets and fonts never sit in memory whole. A
                // read error hashes what was read, like an unreadable file
                // hashes as empty.
                let _ = std::io::copy(&mut file, &mut h);
            }
            h.update(SEPARATOR);
        }
        own.insert(c.name.clone(), h.finalize().to_vec());
    }
    // Fold in dependencies, transitively (sorted, cycle-safe).
    fn closure(name: &str, comps: &[Component], seen: &mut BTreeSet<String>) {
        if !seen.insert(name.to_string()) {
            return;
        }
        if let Some(c) = comps.iter().find(|c| c.name == name) {
            for d in &c.deps {
                closure(d, comps, seen);
            }
        }
    }
    components
        .iter()
        .map(|c| {
            let mut seen = BTreeSet::new();
            closure(&c.name, components, &mut seen);
            let mut h = Sha256::new();
            for name in &seen {
                if let Some(o) = own.get(name) {
                    h.update(name.as_bytes());
                    h.update(o);
                }
            }
            let digest = h.finalize();
            (c.name.clone(), hex(&digest[..HASH_BYTES]))
        })
        .collect()
}

/// Reads `versions.toml`: `[name] version = "…" hash = "…"`.
pub fn read_record(root: &Path) -> BTreeMap<String, Recorded> {
    table(&read(&root.join(RECORD)))
        .into_iter()
        .filter_map(|(k, v)| {
            let t = v.as_table()?;
            Some((k, Recorded { version: t.get("version")?.as_str()?.to_string(), hash: t.get("hash")?.as_str()?.to_string() }))
        })
        .collect()
}

/// Writes `versions.toml` (sorted, with a header).
pub fn write_record(root: &Path, record: &BTreeMap<String, Recorded>, kinds: &BTreeMap<String, String>) -> std::io::Result<()> {
    let mut s = String::from(
        "# Component versions and content hashes — maintained by `cargo xtask bump`.\n# A component whose current hash differs from the one recorded here has\n# changed since its last release and gets its version bumped.\n",
    );
    for (name, r) in record {
        s.push_str(&format!("\n[{name}]\nkind = \"{}\"\nversion = \"{}\"\nhash = \"{}\"\n", kinds.get(name).map(String::as_str).unwrap_or("library"), r.version, r.hash));
    }
    std::fs::write(root.join(RECORD), s)
}

/// Rewrites the `version = "…"` of `[package]` (or the top-level one in
/// plugin.toml) in `manifest`. Workspace inheritance becomes explicit.
pub fn set_manifest_version(path: &Path, version: &str) -> std::io::Result<()> {
    let text = read(path);
    let is_plugin = path.file_name().is_some_and(|n| n == "plugin.toml");
    let mut out = Vec::new();
    let mut section = String::new();
    let mut done = false;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with('[') {
            section = t.to_string();
        }
        let in_target = if is_plugin { section.is_empty() } else { section == "[package]" };
        if !done && in_target && (t.starts_with("version =") || t.starts_with("version.workspace") || t.starts_with("version=")) {
            out.push(format!("version = \"{version}\""));
            done = true;
        } else {
            out.push(line.to_string());
        }
    }
    let mut s = out.join("\n");
    if text.ends_with('\n') {
        s.push('\n');
    }
    std::fs::write(path, s)
}

/// Semantic version bump.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Bump {
    Patch,
    Minor,
    Major,
}

/// `1.4.2` + Minor → `1.5.0`. Pre-1.0, a breaking change bumps the minor.
pub fn bump(version: &str, bump: Bump) -> String {
    let core = version.split(['-', '+']).next().unwrap_or("0.0.0");
    let mut parts: Vec<u64> = core.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    parts.resize(3, 0);
    let (ma, mi, pa) = (parts[0], parts[1], parts[2]);
    match (bump, ma) {
        (Bump::Major, 0) => format!("0.{}.0", mi + 1),
        (Bump::Major, _) => format!("{}.0.0", ma + 1),
        (Bump::Minor, _) => format!("{ma}.{}.0", mi + 1),
        (Bump::Patch, _) => format!("{ma}.{mi}.{}", pa + 1),
    }
}

/// The bump a set of Conventional Commit subjects/bodies calls for:
/// `feat` → minor, `!` / `BREAKING CHANGE` → major, anything else → patch.
pub fn bump_from_commits(messages: &[String]) -> Bump {
    let mut b = Bump::Patch;
    for m in messages {
        let subject = m.lines().next().unwrap_or("");
        let head = subject.split(':').next().unwrap_or("");
        if head.ends_with('!') || m.contains("BREAKING CHANGE") || m.contains("BREAKING-CHANGE") {
            return Bump::Major;
        }
        if head.starts_with("feat") {
            b = b.max(Bump::Minor);
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_bumps() {
        assert_eq!(bump("1.4.2", Bump::Patch), "1.4.3");
        assert_eq!(bump("1.4.2", Bump::Minor), "1.5.0");
        assert_eq!(bump("1.4.2", Bump::Major), "2.0.0");
        assert_eq!(bump("0.3.9", Bump::Major), "0.4.0");
        assert_eq!(bump("0.1.0-dev+abc", Bump::Patch), "0.1.1");
    }

    #[test]
    fn conventional_commits() {
        let m = |s: &str| vec![s.to_string()];
        assert_eq!(bump_from_commits(&m("fix: typo")), Bump::Patch);
        assert_eq!(bump_from_commits(&m("feat(bar): bottom position")), Bump::Minor);
        assert_eq!(bump_from_commits(&m("feat!: new plugin API")), Bump::Major);
        assert_eq!(bump_from_commits(&m("refactor: x\n\nBREAKING CHANGE: y")), Bump::Major);
    }

    #[test]
    fn version_line_rewrite_and_strip() {
        let dir = std::env::temp_dir().join(format!("mandara-version-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("Cargo.toml");
        std::fs::write(&p, "[package]\nname = \"x\"\nversion.workspace = true\n\n[dependencies]\nfoo = { version = \"1\" }\n").unwrap();
        set_manifest_version(&p, "0.2.0").unwrap();
        let t = std::fs::read_to_string(&p).unwrap();
        assert!(t.contains("version = \"0.2.0\"") && t.contains("foo = { version = \"1\" }"));
        assert_eq!(strip_version("a = 1\nversion = \"9\"\nb = 2"), "a = 1\nb = 2");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
