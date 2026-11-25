//! The **source fingerprint**: one SHA-256 over everything that decides
//! what a build produces — the workspace manifests and lockfile, every
//! crate's sources, build scripts, locales and embedded assets, the
//! vendored crates — plus the toolchain (`rustc -vV`), target triple,
//! profile, features and `RUSTFLAGS`.
//!
//! Deterministic by construction: files are hashed by their path relative
//! to the workspace root (always `/`-separated) and their content, sorted
//! by path, so neither directory order, mtimes, the checkout location nor
//! git (present, absent or freshly re-initialized) can change it. Files
//! that are never build inputs (build output, VCS and editor state, agent
//! notes) are skipped by a fixed list rather than `.gitignore`, which is
//! itself local and absent from CI checkouts.

use std::path::{Path, PathBuf};

use mandara_version::manifest;
use sha2::{Digest, Sha256};

/// Domain tag hashed first; bump the suffix when the scheme changes so old
/// and new fingerprints can never collide.
const DOMAIN: &[u8] = b"mandara-source-fingerprint-v1";
/// Separates fields so boundaries can't shift between them.
const SEPARATOR: [u8; 1] = [0];

/// Workspace-root files that are build inputs: manifests, toolchain,
/// `PLUGINS.md` (embedded) and `versions.toml` (embedded versions).
pub const INPUT_FILES: &[&str] = &["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", ".cargo/config.toml", "PLUGINS.md", "versions.toml"];
/// Workspace-root directories whose every file is a build input. `xtask`
/// and `plugins` count too: `mandara-version` embeds every component's
/// content hash, so their sources change the binary's bytes.
pub const INPUT_DIRS: &[&str] = &["crates", "vendor", "assets", "xtask", "plugins", "tools"];

/// Directory names never descended into.
const SKIPPED_DIRS: &[&str] = &["target", "dist", "node_modules", "memory", "memories"];
/// File names (compared case-insensitively) that are never build inputs.
const SKIPPED_FILES: &[&str] = &["claude.md", "claude.local.md", "agents.md", "agent.md", "gemini.md", ".ds_store"];
/// File-name suffixes of editor and tool leftovers.
const SKIPPED_SUFFIXES: &[&str] = &[".rs.bk", ".pdb", ".swp", ".swo", "~", ".jsonl", ".orig", ".rej"];

/// Whether a directory entry called `name` is skipped.
fn skipped(name: &str, is_dir: bool) -> bool {
    // Hidden entries are VCS / editor / agent state, except `.cargo`.
    if name.starts_with('.') && name != ".cargo" {
        return true;
    }
    if is_dir {
        return SKIPPED_DIRS.contains(&name);
    }
    let lower = name.to_ascii_lowercase();
    SKIPPED_FILES.contains(&lower.as_str()) || SKIPPED_SUFFIXES.iter().any(|s| lower.ends_with(s))
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        let p = e.path();
        // Symlinked folders are followed, like the component hashes do.
        let is_dir = p.is_dir();
        if skipped(&name, is_dir) {
            continue;
        }
        if is_dir {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// Every build-input file under `root`, as absolute paths.
pub fn input_files(root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = INPUT_FILES.iter().map(|f| root.join(f)).filter(|p| p.is_file()).collect();
    for d in INPUT_DIRS {
        walk(&root.join(d), &mut out);
    }
    out
}

/// `/`-separated path of `path` relative to `root`.
pub fn relative(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/// Toolchain and configuration the build is made with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildConfig {
    /// `rustc -vV`, trimmed.
    pub rustc: String,
    pub target: String,
    pub profile: String,
    /// Enabled features of the binary crate, sorted.
    pub features: Vec<String>,
    pub default_features: bool,
    /// `RUSTFLAGS` (or `CARGO_ENCODED_RUSTFLAGS`).
    pub rustflags: String,
}

impl BuildConfig {
    /// `key=value` lines in a fixed order.
    fn fields(&self) -> Vec<(&'static str, String)> {
        let mut features = self.features.clone();
        features.sort();
        features.dedup();
        vec![
            ("rustc", self.rustc.trim().to_string()),
            ("target", self.target.clone()),
            ("profile", self.profile.clone()),
            ("features", features.join(",")),
            ("default-features", self.default_features.to_string()),
            ("rustflags", self.rustflags.trim().to_string()),
        ]
    }
}

/// A computed fingerprint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fingerprint {
    /// SHA-256, lowercase hex.
    pub full: String,
    pub files: u64,
    pub bytes: u64,
}

impl Fingerprint {
    pub fn short(&self) -> &str {
        &self.full[..manifest::FINGERPRINT_SHORT_LEN.min(self.full.len())]
    }
}

/// One input: relative path and the SHA-256 of its content.
pub type Entry = (String, [u8; 32]);

/// Folds `entries` (any order) and `config` into the fingerprint.
pub fn combine(mut entries: Vec<Entry>, config: &BuildConfig) -> String {
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut h = Sha256::new();
    h.update(DOMAIN);
    h.update(SEPARATOR);
    for (path, digest) in &entries {
        h.update(path.as_bytes());
        h.update(SEPARATOR);
        h.update(digest);
    }
    for (key, value) in config.fields() {
        h.update(key.as_bytes());
        h.update(b"=");
        h.update(value.as_bytes());
        h.update(SEPARATOR);
    }
    manifest::hex(&h.finalize())
}

/// SHA-256 of one file's content, streamed.
fn file_digest(path: &Path) -> std::io::Result<([u8; 32], u64)> {
    let mut file = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let bytes = std::io::copy(&mut file, &mut h)?;
    Ok((h.finalize().into(), bytes))
}

/// Fingerprints the workspace at `root` built with `config`.
pub fn compute(root: &Path, config: &BuildConfig) -> Result<Fingerprint, String> {
    let files = input_files(root);
    let mut entries = Vec::with_capacity(files.len());
    let mut bytes = 0;
    for f in &files {
        let (digest, len) = file_digest(f).map_err(|e| format!("{}: {e}", f.display()))?;
        bytes += len;
        entries.push((relative(root, f), digest));
    }
    let count = entries.len() as u64;
    Ok(Fingerprint { full: combine(entries, config), files: count, bytes })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(content: &[u8]) -> [u8; 32] {
        Sha256::digest(content).into()
    }

    fn config() -> BuildConfig {
        BuildConfig {
            rustc: "rustc 1.99.1 (58cf8f9c3 2026-10-13)\nhost: x86_64-unknown-linux-gnu".into(),
            target: "x86_64-unknown-linux-gnu".into(),
            profile: "release".into(),
            features: vec!["alloc-system-tuned".into()],
            default_features: true,
            rustflags: String::new(),
        }
    }

    fn entries() -> Vec<Entry> {
        vec![("Cargo.toml".into(), digest(b"[workspace]")), ("crates/a/src/lib.rs".into(), digest(b"pub fn a() {}")), ("crates/b/build.rs".into(), digest(b"fn main() {}"))]
    }

    #[test]
    fn same_inputs_same_hash() {
        let a = combine(entries(), &config());
        assert_eq!(a, combine(entries(), &config()));
        assert_eq!(a.len(), manifest::FINGERPRINT_HEX_LEN);
    }

    #[test]
    fn order_independent() {
        let mut reversed = entries();
        reversed.reverse();
        assert_eq!(combine(entries(), &config()), combine(reversed, &config()));
        let mut c = config();
        c.features = vec!["b".into(), "a".into()];
        let mut d = config();
        d.features = vec!["a".into(), "b".into()];
        assert_eq!(combine(entries(), &c), combine(entries(), &d));
    }

    #[test]
    fn content_path_and_config_changes_change_the_hash() {
        let base = combine(entries(), &config());
        let mut edited = entries();
        edited[1].1 = digest(b"pub fn a() { }");
        assert_ne!(base, combine(edited, &config()));
        let mut renamed = entries();
        renamed[1].0 = "crates/a/src/main.rs".into();
        assert_ne!(base, combine(renamed, &config()));
        let mut added = entries();
        added.push(("crates/a/locales/ar.toml".into(), digest(b"")));
        assert_ne!(base, combine(added, &config()));
        for change in [
            |c: &mut BuildConfig| c.profile = "dev".into(),
            |c: &mut BuildConfig| c.target = "aarch64-unknown-linux-gnu".into(),
            |c: &mut BuildConfig| c.rustc = "rustc 1.100.0".into(),
            |c: &mut BuildConfig| c.features.push("alloc-jemalloc".into()),
            |c: &mut BuildConfig| c.default_features = false,
            |c: &mut BuildConfig| c.rustflags = "-C target-cpu=x86-64-v3".into(),
        ] {
            let mut c = config();
            change(&mut c);
            assert_ne!(base, combine(entries(), &c));
        }
    }

    #[test]
    fn path_boundaries_cannot_shift() {
        // "ab" + content vs "a" + "b…": separators keep these apart.
        let a = combine(vec![("ab".into(), digest(b"x"))], &config());
        let b = combine(vec![("a".into(), digest(b"bx"))], &config());
        assert_ne!(a, b);
    }

    #[test]
    fn tree_fingerprint_ignores_mtimes_and_skipped_files() {
        let dir = std::env::temp_dir().join(format!("mandara-fingerprint-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("crates/demo/src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[workspace]\n").unwrap();
        std::fs::write(src.join("lib.rs"), "pub fn demo() {}\n").unwrap();
        let first = compute(&dir, &config()).unwrap();
        assert_eq!(first.files, 2);
        // Rewriting identical content (new mtime) and adding skipped files
        // (build output, editor backups, agent notes) changes nothing.
        std::fs::write(src.join("lib.rs"), "pub fn demo() {}\n").unwrap();
        std::fs::create_dir_all(dir.join("crates/demo/target")).unwrap();
        std::fs::write(dir.join("crates/demo/target/out.rlib"), "binary").unwrap();
        std::fs::write(src.join("lib.rs~"), "backup").unwrap();
        std::fs::write(dir.join("crates/demo/CLAUDE.md"), "notes").unwrap();
        std::fs::create_dir_all(dir.join("crates/demo/.git")).unwrap();
        std::fs::write(dir.join("crates/demo/.git/HEAD"), "ref").unwrap();
        assert_eq!(compute(&dir, &config()).unwrap(), first);
        // A real edit does change it.
        std::fs::write(src.join("lib.rs"), "pub fn demo() -> u8 { 1 }\n").unwrap();
        assert_ne!(compute(&dir, &config()).unwrap().full, first.full);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn relative_paths_use_forward_slashes() {
        let root = Path::new("/w");
        assert_eq!(relative(root, Path::new("/w/crates/a/src/lib.rs")), "crates/a/src/lib.rs");
    }
}
