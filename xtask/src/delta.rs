//! Delta updates (format and patching in `tools/mandara-delta`):
//!
//! - `delta --old BIN --new BIN --out FILE`: a compressed patch, verified by
//!   applying it with the updater's pure-Rust decoder.
//! - `apply-delta --old BIN --patch FILE --out BIN [--sha256 HEX]`.
//! - `update-part --target <triple> [--variant NAME] [--out dist]
//!   [--previous OLD.tar.gz]`: after `dist`, this target's `update.json`
//!   entry (`mandara-<v>-<target>.update-part.json`; the release job merges
//!   the parts) and, given the previous release's tarball of the same
//!   target, the patch `mandara-<old>-to-<v>-<target>.patch.zst`.
//! - `plugin-delta --new <id>.mandara.tar.gz [--old PREVIOUS] --out DIR`:
//!   a plugin release's `update.json`, meta archive and patch.

use std::path::{Path, PathBuf};

use mandara_delta::{Manifest, SCHEMA, SHELL_BINARY};

use crate::{Result, opt, root, short_target, state};

/// Name of the shell in manifests and asset names.
const SHELL_NAME: &str = "mandara";
/// Largest binary read from a release tarball.
const MAX_BINARY: u64 = 1 << 30;
/// Suffix of a per-target manifest part.
const PART_SUFFIX: &str = ".update-part.json";

fn need(args: &[String], name: &str) -> Result<PathBuf> {
    opt(args, name).map(PathBuf::from).ok_or_else(|| format!("{name} is required"))
}

fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn write(path: &Path, data: &[u8]) -> Result<()> {
    std::fs::write(path, data).map_err(|e| format!("{}: {e}", path.display()))
}

fn percent(part: usize, whole: usize) -> f64 {
    const PERCENT: f64 = 100.0;
    part as f64 * PERCENT / whole.max(1) as f64
}

pub fn delta(args: &[String]) -> Result<()> {
    let (old, new) = (read(&need(args, "--old")?)?, read(&need(args, "--new")?)?);
    let patch = mandara_delta::create::diff_verified(&old, &new)?;
    write(&need(args, "--out")?, &patch)?;
    println!("patch: {} bytes for a {} byte binary ({:.2}%), sha256 {}", patch.len(), new.len(), percent(patch.len(), new.len()), mandara_delta::sha256_hex(&patch));
    Ok(())
}

pub fn apply_delta(args: &[String]) -> Result<()> {
    let (old, patch) = (read(&need(args, "--old")?)?, read(&need(args, "--patch")?)?);
    let new = mandara_delta::apply(&old, &patch, MAX_BINARY).map_err(|e| e.to_string())?;
    let sha = mandara_delta::sha256_hex(&new);
    if let Some(want) = opt(args, "--sha256")
        && !want.eq_ignore_ascii_case(&sha)
    {
        return Err(format!("result sha256 {sha} doesn't match {want}"));
    }
    write(&need(args, "--out")?, &new)?;
    println!("{} bytes, sha256 {sha}", new.len());
    Ok(())
}

fn tarball_binary(path: &Path) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    mandara_delta::archive::extract_file(std::io::BufReader::new(file), SHELL_BINARY, MAX_BINARY).map_err(|e| format!("{}: {e}", path.display()))?.ok_or_else(|| format!("{}: no {SHELL_BINARY}", path.display()))
}

pub fn update_part(args: &[String]) -> Result<()> {
    let target = opt(args, "--target").unwrap_or_else(|| format!("{}-unknown-linux-gnu", std::env::consts::ARCH));
    let out = root().join(opt(args, "--out").unwrap_or_else(|| "dist".into()));
    let version = state().record.get("mandara-app").map(|r| r.version.clone()).unwrap_or_else(|| "0.0.0".into());
    let key = format!("{}{}", short_target(&target), opt(args, "--variant").map(|v| format!("-{v}")).unwrap_or_default());
    let archive_name = format!("{SHELL_NAME}-{version}-{key}.tar.gz");
    let archive_path = out.join(&archive_name);
    let archive = (archive_name.clone(), read(&archive_path).map_err(|e| format!("{e} — run `cargo xtask dist` first"))?);
    let new_bin = tarball_binary(&archive_path)?;
    // The previous release's tarball of the same target: its version is in
    // its name (`mandara-<old>-<key>.tar.gz`).
    let previous = match opt(args, "--previous").map(PathBuf::from) {
        Some(p) => {
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let old_version = name.strip_prefix(&format!("{SHELL_NAME}-")).and_then(|n| n.strip_suffix(&format!("-{key}.tar.gz"))).ok_or_else(|| format!("{name} is not a {key} release tarball"))?.to_string();
            if old_version == version {
                println!("previous release is {version} itself (a rebuild): no patch");
                None
            } else {
                Some((old_version, tarball_binary(&p)?))
            }
        }
        None => None,
    };
    let (entry, patch) = mandara_delta::create::target_entry(SHELL_NAME, &version, Some(&key), &archive, SHELL_BINARY, &new_bin, previous.as_ref().map(|(v, b)| (v.as_str(), b.as_slice())))?;
    if let Some((file, data)) = &patch {
        write(&out.join(file), data)?;
        println!("{file}: {} bytes ({:.2}% of the {} byte tarball)", data.len(), percent(data.len(), archive.1.len()), archive.1.len());
    } else {
        println!("no previous {key} release: full update only");
    }
    let mut part = Manifest { schema: SCHEMA, name: SHELL_NAME.into(), version: version.clone(), ..Default::default() };
    part.targets.insert(key.clone(), entry);
    let part_path = out.join(format!("{SHELL_NAME}-{version}-{key}{PART_SUFFIX}"));
    write(&part_path, part.to_json().as_bytes())?;
    println!("{}", part_path.display());
    Ok(())
}

pub fn plugin_delta(args: &[String]) -> Result<()> {
    let new_path = need(args, "--new")?;
    let name = new_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let new = (name, read(&new_path)?);
    let old = opt(args, "--old").map(|p| read(Path::new(&p))).transpose()?;
    let out = need(args, "--out")?;
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let (manifest, assets) = mandara_delta::create::plugin_release(&new, old.as_deref())?;
    for (file, data) in &assets {
        write(&out.join(file), data)?;
        println!("{} ({} bytes)", out.join(file).display(), data.len());
    }
    write(&out.join(mandara_delta::MANIFEST_FILE), manifest.to_json().as_bytes())?;
    println!("{}", out.join(mandara_delta::MANIFEST_FILE).display());
    Ok(())
}
