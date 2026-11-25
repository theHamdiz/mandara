//! `mandara-delta`: binary patches and update manifests for Mandara Shell
//! and its plugins.
//!
//! - `mandara-delta diff --old OLD --new NEW --out FILE.patch.zst`
//! - `mandara-delta apply --old OLD --patch FILE.patch.zst --out NEW [--sha256 HEX]`
//! - `mandara-delta plugin --new <id>.mandara.tar.gz [--old PREVIOUS.mandara.tar.gz] --out DIR`
//!   writes `update.json`, the meta archive and (with `--old`) the patch
//!   into DIR, ready to upload with the plugin's release.

use std::path::{Path, PathBuf};

/// Exit code for a usage error.
const EXIT_USAGE: i32 = 2;
/// Exit code for a failed command.
const EXIT_FAILURE: i32 = 1;
const USAGE: &str = "usage: mandara-delta <diff --old OLD --new NEW --out PATCH | apply --old OLD --patch PATCH --out NEW [--sha256 HEX] | plugin --new ARCHIVE [--old ARCHIVE] --out DIR>";

fn opt(args: &[String], name: &str) -> Option<PathBuf> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(PathBuf::from)
}

fn need(args: &[String], name: &str) -> Result<PathBuf, String> {
    opt(args, name).ok_or_else(|| format!("{name} is required\n{USAGE}"))
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn write(path: &Path, data: &[u8]) -> Result<(), String> {
    std::fs::write(path, data).map_err(|e| format!("{}: {e}", path.display()))
}

fn run(args: &[String]) -> Result<(), String> {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("diff") => {
            let (old, new) = (read(&need(rest, "--old")?)?, read(&need(rest, "--new")?)?);
            let patch = mandara_delta::create::diff_verified(&old, &new)?;
            write(&need(rest, "--out")?, &patch)?;
            println!("patch: {} bytes for a {} byte file ({:.1}%), sha256 {}", patch.len(), new.len(), patch.len() as f64 * 100.0 / new.len().max(1) as f64, mandara_delta::sha256_hex(&patch));
            Ok(())
        }
        Some("apply") => {
            let (old, patch) = (read(&need(rest, "--old")?)?, read(&need(rest, "--patch")?)?);
            let new = mandara_delta::apply(&old, &patch, u64::MAX).map_err(|e| e.to_string())?;
            let sha = mandara_delta::sha256_hex(&new);
            if let Some(want) = rest.iter().position(|a| a == "--sha256").and_then(|i| rest.get(i + 1))
                && !want.eq_ignore_ascii_case(&sha)
            {
                return Err(format!("result sha256 {sha} doesn't match {want}"));
            }
            write(&need(rest, "--out")?, &new)?;
            println!("{} bytes, sha256 {sha}", new.len());
            Ok(())
        }
        Some("plugin") => {
            let new_path = need(rest, "--new")?;
            let name = new_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let new = (name, read(&new_path)?);
            let old = opt(rest, "--old").map(|p| read(&p)).transpose()?;
            let out = need(rest, "--out")?;
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
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(EXIT_USAGE);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(&args) {
        eprintln!("error: {e}");
        std::process::exit(EXIT_FAILURE);
    }
}
