//! `cargo xtask build`: a fingerprinted release build.
//!
//! 1. Fingerprints the sources, toolchain and build configuration
//!    ([`fingerprint`](crate::fingerprint)).
//! 2. Builds `mandara-app` with the fingerprint handed to
//!    `mandara-version`'s build script, so `mandara --version` reports it.
//! 3. Hashes every shipped binary (SHA-256), checks the embedded
//!    fingerprint, and counts the lines of code ([`loc`](crate::loc)).
//! 4. Writes `<out>/mandara-<version>-<target>[-<variant>].build.toml` and,
//!    unless `--no-readme`, refreshes the block between the build-info
//!    markers in `README.md`.

use std::path::{Path, PathBuf};
use std::process::Command;

use mandara_version::manifest;
use serde::Serialize;

use crate::fingerprint::{self, BuildConfig};
use crate::loc::{self, Group};
use crate::{Result, flag, opt, root, run, sha256_file, short_target, state};

/// The binary crate that is built.
const APP_PACKAGE: &str = "mandara-app";
/// Binaries the release ships, all produced by [`APP_PACKAGE`].
const SHIPPED_BINARIES: &[&str] = &["mandara"];
/// Profile built when `--profile` is not given.
const DEFAULT_PROFILE: &str = "release";
/// Manifest format version; bump when fields change meaning.
const MANIFEST_SCHEMA: u32 = 1;
/// Suffix of the build manifest after the release name.
const MANIFEST_SUFFIX: &str = ".build.toml";
/// README markers around the generated block.
pub const README_START: &str = "<!-- build-info:start -->";
pub const README_END: &str = "<!-- build-info:end -->";
/// Heading of the section created when the markers are missing.
const README_SECTION: &str = "## Build fingerprint";
/// The section the build-info section is inserted before.
const README_ANCHOR: &str = "## License";
/// Hex digits of a binary hash shown in the README table.
const BINARY_HASH_SHOWN: usize = 16;
/// Separator byte cargo uses in `CARGO_ENCODED_RUSTFLAGS`.
const ENCODED_RUSTFLAGS_SEPARATOR: char = '\u{1f}';

/// `rustc -vV` with the workspace's toolchain.
fn rustc_verbose() -> Result<String> {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let out = Command::new(&rustc).arg("-vV").current_dir(root()).output().map_err(|e| format!("{rustc} -vV: {e}"))?;
    if !out.status.success() {
        return Err(format!("{rustc} -vV failed ({})", out.status));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The `key: value` line of `rustc -vV` output.
fn rustc_field<'a>(verbose: &'a str, key: &str) -> Option<&'a str> {
    verbose.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix(':').map(str::trim))
}

/// Features enabled by default in the app's manifest.
fn default_features(root: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(root.join("crates").join(APP_PACKAGE).join("Cargo.toml")).unwrap_or_default();
    let table: toml::Table = toml::from_str(&text).unwrap_or_default();
    table.get("features").and_then(|f| f.get("default")).and_then(|d| d.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
}

/// `--features a,b` / `--features "a b"`.
fn requested_features(args: &[String]) -> Vec<String> {
    opt(args, "--features").map(|f| f.split([',', ' ']).filter(|s| !s.is_empty()).map(String::from).collect()).unwrap_or_default()
}

fn rustflags() -> String {
    std::env::var("RUSTFLAGS").ok().filter(|f| !f.trim().is_empty()).or_else(|| std::env::var("CARGO_ENCODED_RUSTFLAGS").ok().map(|f| f.replace(ENCODED_RUSTFLAGS_SEPARATOR, " "))).unwrap_or_default()
}

/// Cargo's output folder for `profile`.
fn profile_dir(profile: &str) -> &str {
    match profile {
        "dev" | "test" => "debug",
        "bench" => "release",
        other => other,
    }
}

/// The shell's version as the binary reports it: the recorded version, or
/// the next patch as `-dev` when its sources changed (as `mandara-version`'s
/// build script decides).
fn app_version() -> String {
    let s = state();
    let Some(c) = s.components.iter().find(|c| c.name == APP_PACKAGE) else { return env!("CARGO_PKG_VERSION").to_string() };
    let changed = s.record.get(&c.name).is_none_or(|r| Some(&r.hash) != s.hashes.get(&c.name));
    if changed { format!("{}-dev", manifest::bump(&c.version, manifest::Bump::Patch)) } else { c.version.clone() }
}

/// Seconds since the epoch: `SOURCE_DATE_EPOCH` for reproducible
/// releases, otherwise now.
fn build_time() -> i64 {
    std::env::var("SOURCE_DATE_EPOCH").ok().and_then(|s| s.parse().ok()).unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0))
}

/// `2026-10-28T12:34:56Z`.
fn utc_timestamp(unix: i64) -> String {
    const SECS_PER_DAY: i64 = 86_400;
    const SECS_PER_HOUR: i64 = 3_600;
    const SECS_PER_MINUTE: i64 = 60;
    let (y, m, d) = mandara_version::civil_from_days(unix.div_euclid(SECS_PER_DAY));
    let secs = unix.rem_euclid(SECS_PER_DAY);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / SECS_PER_HOUR, secs % SECS_PER_HOUR / SECS_PER_MINUTE, secs % SECS_PER_MINUTE)
}

/// `1234567` → `1,234,567`.
fn thousands(n: u64) -> String {
    const GROUP: usize = 3;
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / GROUP);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % GROUP == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[derive(Serialize)]
struct Source {
    /// SHA-256 over every build input and the build configuration.
    fingerprint: String,
    short: String,
    files: u64,
    bytes: u64,
    /// Workspace paths the fingerprint walks.
    inputs: Vec<String>,
}

#[derive(Serialize)]
struct Toolchain {
    rustc: String,
    llvm: String,
    host: String,
    target: String,
    profile: String,
    features: Vec<String>,
    default_features: bool,
    rustflags: String,
    /// Full `rustc -vV`.
    rustc_verbose: String,
}

#[derive(Serialize)]
struct Binary {
    name: String,
    path: String,
    sha256: String,
    bytes: u64,
}

#[derive(Serialize)]
struct BuildManifest {
    schema: u32,
    version: String,
    /// `mandara --version` of the built binary.
    version_string: String,
    built_utc: String,
    source: Source,
    toolchain: Toolchain,
    binary: Vec<Binary>,
    lines: loc::Report,
}

/// `1 file`, `2 files`.
fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", thousands(n), if n == 1 { one } else { many })
}

fn lines_cell(g: &Group) -> String {
    format!("{} code · {} comments · {} blank", thousands(g.lines.code), thousands(g.lines.comments), thousands(g.lines.blank))
}

/// The Markdown between the README markers.
fn readme_block(m: &BuildManifest) -> String {
    let t = &m.toolchain;
    let features = if t.features.is_empty() { "none".to_string() } else { t.features.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ") };
    let features = format!("{features}{}", if t.default_features { " (default)" } else { "" });
    let mut out = String::new();
    out.push_str("<!-- Generated by `cargo xtask build`; edits here are overwritten. -->\n");
    out.push_str("| Build | |\n|---|---|\n");
    out.push_str(&format!("| Version | `{}` |\n", m.version));
    out.push_str(&format!("| Source fingerprint | `{}` |\n", m.source.short));
    out.push_str(&format!("| Toolchain | `{}`, LLVM {} |\n", t.rustc, t.llvm));
    out.push_str(&format!("| Target · profile | `{}` · `{}` |\n", t.target, t.profile));
    out.push_str(&format!("| Features | {features} |\n"));
    for b in &m.binary {
        out.push_str(&format!("| `{}` SHA-256 | `{}…` |\n", b.name, &b.sha256[..BINARY_HASH_SHOWN.min(b.sha256.len())]));
    }
    let l = &m.lines;
    out.push_str(&format!("| Rust | {} · {} · {} |\n", lines_cell(&l.rust), plural(l.crates.len() as u64, "crate", "crates"), plural(l.rust.files, "file", "files")));
    out.push_str(&format!("| Vendored Rust | {} · {} |\n", lines_cell(&l.vendored), plural(l.vendored.files, "file", "files")));
    out.push_str(&format!("| Plugin examples | {} · {} |\n", lines_cell(&l.plugins), plural(l.plugins.files, "file", "files")));
    out.push_str(&format!("| Locales · WIT | {} lines in {} · {} lines in {} |\n", thousands(l.locales.lines.code), plural(l.locales.files, "catalog", "catalogs"), thousands(l.wit.lines.code), plural(l.wit.files, "WIT file", "WIT files")));
    out.push_str(&format!("| Built | {} |\n", m.built_utc.replace('T', " ").replace('Z', " UTC")));
    out.push_str("\n<details><summary>Full hashes and lines per crate</summary>\n\n```text\n");
    let width = m.binary.iter().map(|b| b.name.len()).chain([SOURCE_LABEL.len()]).max().unwrap_or(0);
    out.push_str(&format!("{SOURCE_LABEL:<width$}  {}\n", m.source.fingerprint));
    for b in &m.binary {
        out.push_str(&format!("{:<width$}  {}\n", b.name, b.sha256));
    }
    out.push_str("```\n\n| Crate | Files | Code | Comments | Blank |\n|---|---:|---:|---:|---:|\n");
    for c in &l.crates {
        out.push_str(&format!("| `{}` | {} | {} | {} | {} |\n", c.name, c.files, thousands(c.lines.code), thousands(c.lines.comments), thousands(c.lines.blank)));
    }
    out.push_str(&format!(
        "| **Total** | **{}** | **{}** | **{}** | **{}** |\n\n</details>\n",
        thousands(l.rust.files),
        thousands(l.rust.lines.code),
        thousands(l.rust.lines.comments),
        thousands(l.rust.lines.blank)
    ));
    out
}

/// Label of the fingerprint line in the full-hash listing.
const SOURCE_LABEL: &str = "source fingerprint";

/// `readme` with `block` between the markers. Missing markers get a
/// `## Build fingerprint` section before `## License` (or at the end);
/// nothing else changes.
pub fn splice_readme(readme: &str, block: &str) -> String {
    if let (Some(start), Some(end)) = (readme.find(README_START), readme.find(README_END)) {
        if start < end {
            let head = &readme[..start + README_START.len()];
            let tail = &readme[end..];
            return format!("{head}\n{block}{tail}");
        }
    }
    let section = format!(
        "{README_SECTION}\n\nThe last `cargo xtask build`: source fingerprint, toolchain, binary hashes and\nlines of code ([`docs/releasing.md`](docs/releasing.md#build-fingerprint)).\n\n{README_START}\n{block}{README_END}\n\n"
    );
    let anchor = readme.match_indices(README_ANCHOR).find(|(i, _)| *i == 0 || readme.as_bytes()[i - 1] == b'\n').map(|(i, _)| i);
    match anchor {
        Some(i) => format!("{}{section}{}", &readme[..i], &readme[i..]),
        None => format!("{}\n\n{}", readme.trim_end(), section.trim_end()) + "\n",
    }
}

pub fn build(args: &[String]) -> Result<()> {
    let root = root();
    let verbose = rustc_verbose()?;
    let host = rustc_field(&verbose, "host").unwrap_or_default().to_string();
    let explicit_target = opt(args, "--target");
    let target = explicit_target.clone().unwrap_or_else(|| host.clone());
    let profile = opt(args, "--profile").unwrap_or_else(|| DEFAULT_PROFILE.into());
    let default_on = !flag(args, "--no-default-features");
    let mut features = requested_features(args);
    if default_on {
        features.extend(default_features(&root));
    }
    features.sort();
    features.dedup();
    let config = BuildConfig { rustc: verbose.clone(), target: target.clone(), profile: profile.clone(), features: features.clone(), default_features: default_on, rustflags: rustflags() };

    let fp = fingerprint::compute(&root, &config)?;
    println!("source fingerprint {} ({} files, {} bytes)", fp.full, thousands(fp.files), thousands(fp.bytes));

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.args(["build", "--locked", "--profile", &profile, "-p", APP_PACKAGE]).env(manifest::FINGERPRINT_ENV, &fp.full);
    if let Some(t) = &explicit_target {
        cmd.args(["--target", t]);
    }
    if !default_on {
        cmd.arg("--no-default-features");
    }
    let requested = requested_features(args);
    if !requested.is_empty() {
        cmd.args(["--features", &requested.join(",")]);
    }
    run(&mut cmd)?;

    let target_dir = std::env::var("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|_| root.join("target"));
    let out_dir = match &explicit_target {
        Some(t) => target_dir.join(t).join(profile_dir(&profile)),
        None => target_dir.join(profile_dir(&profile)),
    };
    let mut binaries = Vec::new();
    for name in SHIPPED_BINARIES {
        let path = out_dir.join(name);
        let bytes = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?.len();
        binaries.push(Binary { name: (*name).to_string(), path: fingerprint::relative(&root, &path), sha256: sha256_file(&path)?, bytes });
    }

    // The binary must report the fingerprint it was built from.
    let main_bin = out_dir.join(SHIPPED_BINARIES[0]);
    let version_string = match Command::new(&main_bin).arg("--version").output() {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !s.contains(fp.short()) {
                let stderr = String::from_utf8_lossy(&o.stderr);
                return Err(format!(
                    "{} --version does not report fingerprint {}: {s:?} ({}, stderr: {:?})",
                    main_bin.display(),
                    fp.short(),
                    o.status,
                    stderr.trim()
                ));
            }
            s
        }
        // Cross builds can't run here; the fingerprint is still embedded.
        Err(e) => {
            eprintln!("warning: could not run {} --version: {e}", main_bin.display());
            String::new()
        }
    };

    let version = app_version();
    let m = BuildManifest {
        schema: MANIFEST_SCHEMA,
        version: version.clone(),
        version_string,
        built_utc: utc_timestamp(build_time()),
        source: Source {
            short: fp.short().to_string(),
            fingerprint: fp.full.clone(),
            files: fp.files,
            bytes: fp.bytes,
            inputs: fingerprint::INPUT_FILES.iter().chain(fingerprint::INPUT_DIRS).map(|s| s.to_string()).collect(),
        },
        toolchain: Toolchain {
            rustc: verbose.lines().next().unwrap_or_default().to_string(),
            llvm: rustc_field(&verbose, "LLVM version").unwrap_or_default().to_string(),
            host,
            target: target.clone(),
            profile,
            features,
            default_features: default_on,
            rustflags: config.rustflags.clone(),
            rustc_verbose: verbose,
        },
        binary: binaries,
        lines: loc::report(&root),
    };

    let out = root.join(opt(args, "--out").unwrap_or_else(|| "dist".into()));
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let suffix = opt(args, "--variant").map(|v| format!("-{v}")).unwrap_or_default();
    let manifest_path = out.join(format!("mandara-{version}-{}{suffix}{MANIFEST_SUFFIX}", short_target(&target)));
    let text = toml::to_string_pretty(&m).map_err(|e| e.to_string())?;
    let header = "# Build manifest written by `cargo xtask build` (docs/releasing.md).\n";
    std::fs::write(&manifest_path, format!("{header}{text}")).map_err(|e| format!("{}: {e}", manifest_path.display()))?;

    for b in &m.binary {
        println!("{}  {}", b.sha256, b.path);
    }
    let l = &m.lines;
    println!("lines: rust {} total, {} ({} crates, {} files), vendored {}, plugins {}", thousands(l.rust.lines.total()), lines_cell(&l.rust), l.crates.len(), l.rust.files, lines_cell(&l.vendored), lines_cell(&l.plugins));
    println!("{}", manifest_path.display());

    if !flag(args, "--no-readme") {
        let readme = root.join("README.md");
        let text = std::fs::read_to_string(&readme).map_err(|e| format!("{}: {e}", readme.display()))?;
        let updated = splice_readme(&text, &readme_block(&m));
        if updated != text {
            std::fs::write(&readme, updated).map_err(|e| format!("{}: {e}", readme.display()))?;
        }
        println!("{} (build-info block)", readme.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn timestamps() {
        assert_eq!(utc_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_timestamp(1_793_107_696), "2026-10-27T13:28:16Z");
    }

    #[test]
    fn readme_markers_are_created_before_license() {
        let readme = "# Mandara\n\nIntro.\n\n## License\n\nGPL.\n";
        let once = splice_readme(readme, "| a |\n");
        assert!(once.starts_with("# Mandara\n\nIntro.\n\n## Build fingerprint\n"));
        assert!(once.contains(&format!("{README_START}\n| a |\n{README_END}\n\n## License\n\nGPL.\n")));
        // Re-running replaces only the block.
        let twice = splice_readme(&once, "| b |\n");
        assert_eq!(twice, once.replace("| a |", "| b |"));
    }

    #[test]
    fn readme_without_license_appends() {
        let out = splice_readme("# Title\n", "x\n");
        assert!(out.ends_with(&format!("{README_START}\nx\n{README_END}\n")));
    }

    #[test]
    fn rustc_fields() {
        let v = "rustc 1.99.1 (58cf8f9c3 2026-10-13)\nhost: x86_64-unknown-linux-gnu\nLLVM version: 23.1.1";
        assert_eq!(rustc_field(v, "host"), Some("x86_64-unknown-linux-gnu"));
        assert_eq!(rustc_field(v, "LLVM version"), Some("23.1.1"));
    }
}
