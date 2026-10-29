//! Embeds the build's identity: every component's recorded version, its
//! current content hash, whether it changed since that version was
//! recorded (→ a `-dev` pre-release of the next patch), the git revision
//! and the build date.

#[path = "src/manifest.rs"]
#[allow(dead_code)]
mod manifest;

use std::path::Path;

fn git(args: &[&str], root: &Path) -> Option<String> {
    let out = std::process::Command::new("git").args(args).current_dir(root).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Characters of the git revision embedded (`git rev-parse --short=12`).
const GIT_SHA_LEN: usize = 12;

fn main() {
    let root = Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../..").canonicalize().unwrap();
    let components = manifest::discover(&root);
    let hashes = manifest::hashes(&root, &components);
    let record = manifest::read_record(&root);

    let mut rows = String::new();
    for c in &components {
        let hash = hashes.get(&c.name).cloned().unwrap_or_default();
        let recorded = record.get(&c.name);
        let changed = recorded.is_none_or(|r| r.hash != hash);
        // A changed component reports the next patch as a dev pre-release,
        // so local builds never masquerade as the released version.
        let version = if changed { format!("{}-dev", manifest::bump(&c.version, manifest::Bump::Patch)) } else { c.version.clone() };
        rows.push_str(&format!(
            "    Component {{ name: {:?}, kind: {:?}, version: {:?}, hash: {:?}, changed: {changed} }},\n",
            c.name,
            c.kind.as_str(),
            version,
            hash
        ));
    }
    let sha = std::env::var("GITHUB_SHA").ok().map(|s| s[..s.len().min(GIT_SHA_LEN)].to_string()).or_else(|| git(&["rev-parse", &format!("--short={GIT_SHA_LEN}"), "HEAD"], &root)).unwrap_or_else(|| "unknown".into());
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"], &root).is_some_and(|s| !s.is_empty());
    let date = std::env::var("SOURCE_DATE_EPOCH").ok().and_then(|s| s.parse::<i64>().ok()).unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0));
    let target = std::env::var("TARGET").unwrap_or_default();
    let profile = std::env::var("PROFILE").unwrap_or_default();
    // Set only by `cargo xtask build`; anything but a full lowercase hex
    // digest is ignored so a stray value can't masquerade as one.
    let fingerprint = std::env::var(manifest::FINGERPRINT_ENV)
        .ok()
        .filter(|f| f.len() == manifest::FINGERPRINT_HEX_LEN && f.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .unwrap_or_default();
    let out = format!(
        "pub const COMPONENTS: &[Component] = &[\n{rows}];\npub const GIT_SHA: &str = {sha:?};\npub const GIT_DIRTY: bool = {dirty};\npub const BUILD_UNIX: i64 = {date};\npub const TARGET: &str = {target:?};\npub const PROFILE: &str = {profile:?};\npub const SOURCE_FINGERPRINT: &str = {fingerprint:?};\n"
    );
    std::fs::write(Path::new(&std::env::var("OUT_DIR").unwrap()).join("build_info.rs"), out).unwrap();

    // Rebuild when any component's files or the record change.
    println!("cargo:rerun-if-changed={}", root.join(manifest::RECORD).display());
    println!("cargo:rerun-if-changed={}", root.join("Cargo.lock").display());
    println!("cargo:rerun-if-changed={}", root.join("Cargo.toml").display());
    println!("cargo:rerun-if-changed={}", root.join(".git/HEAD").display());
    for c in &components {
        println!("cargo:rerun-if-changed={}", root.join(&c.dir).display());
    }
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    println!("cargo:rerun-if-env-changed={}", manifest::FINGERPRINT_ENV);
}
