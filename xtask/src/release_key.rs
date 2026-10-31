//! The release signing key (`mandara_delta::signing`):
//!
//! - `release-key new [--repo OWNER/NAME] [--secret NAME] [--backup PATH]
//!   [--force]`: creates an Ed25519 key, hands the private half straight
//!   to `gh secret set` (stdin, never printed or logged), keeps one backup
//!   readable only by you, and adds the public half to
//!   `tools/mandara-delta/release-keys.txt`. Refuses to replace an existing
//!   secret, backup or key without `--force`.
//! - `release-key sign FILE [--env NAME]`: the release job's step. Signs
//!   FILE with the key in the environment (`MANDARA_RELEASE_KEY`), checks
//!   the signature against the shipped keys, and writes `FILE.sig`.

use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use mandara_delta::signing::{self, sign};

use crate::{Result, opt, root};

/// The release workflow's secret.
const SECRET: &str = "MANDARA_RELEASE_KEY";
const REPO: &str = "theHamdiz/mandara";
/// The shipped public keys.
const KEYS_FILE: &str = "tools/mandara-delta/release-keys.txt";
/// Only the owner may read the backup.
const PRIVATE: u32 = 0o600;
const PRIVATE_DIR: u32 = 0o700;

pub fn release_key(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("new") => new(&args[1..]),
        Some("sign") => sign_file(&args[1..]),
        _ => Err("usage: cargo xtask release-key <new [--repo OWNER/NAME] [--secret NAME] [--backup PATH] [--force] | sign FILE [--env NAME]>".into()),
    }
}

fn default_backup() -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"));
    data.join("mandara-release/release-key")
}

fn new(args: &[String]) -> Result<()> {
    let repo = opt(args, "--repo").unwrap_or_else(|| REPO.into());
    let secret = opt(args, "--secret").unwrap_or_else(|| SECRET.into());
    let backup = opt(args, "--backup").map(PathBuf::from).unwrap_or_else(default_backup);
    let force = args.iter().any(|a| a == "--force");
    let keys_file = root().join(KEYS_FILE);
    let listing = std::fs::read_to_string(&keys_file).map_err(|e| format!("{}: {e}", keys_file.display()))?;
    if !force && !signing::parse_keys(&listing).is_empty() {
        return Err(format!("{KEYS_FILE} already has a release key; pass --force to add another (a key change)"));
    }
    if !force && secret_exists(&repo, &secret)? {
        return Err(format!("{repo} already has a {secret} secret; pass --force to replace it"));
    }
    if backup.exists() {
        return Err(format!("{} already exists; move it away first (it may be the only copy of a key)", backup.display()));
    }
    let (private, public) = sign::generate()?;
    write_backup(&backup, &private)?;
    set_secret(&repo, &secret, &private)?;
    let mut listing = listing;
    if !listing.ends_with('\n') {
        listing.push('\n');
    }
    listing.push_str(&format!("{public}  # added {}\n", crate::today()));
    std::fs::write(&keys_file, listing).map_err(|e| format!("{}: {e}", keys_file.display()))?;
    println!("Release key created.");
    println!("  public key  {public} (added to {KEYS_FILE}; commit it)");
    println!("  secret      {secret} on {repo}");
    println!("  backup      {} (only you can read it; keep a copy offline)", backup.display());
    Ok(())
}

fn secret_exists(repo: &str, name: &str) -> Result<bool> {
    let out = Command::new("gh").args(["secret", "list", "--repo", repo]).output().map_err(|e| format!("gh: {e}"))?;
    if !out.status.success() {
        return Err(format!("gh secret list: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).lines().any(|l| l.split_whitespace().next() == Some(name)))
}

fn write_backup(path: &Path, private: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::DirBuilder::new().recursive(true).mode(PRIVATE_DIR).create(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(PRIVATE).open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(format!("{private}\n").as_bytes()).and_then(|_| f.sync_all()).map_err(|e| format!("{}: {e}", path.display()))
}

/// `gh secret set` with the value on stdin (never on the command line).
fn set_secret(repo: &str, name: &str, private: &str) -> Result<()> {
    let mut child = Command::new("gh").args(["secret", "set", name, "--repo", repo]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| format!("gh: {e}"))?;
    child.stdin.take().ok_or("gh: no stdin")?.write_all(private.as_bytes()).map_err(|e| format!("gh: {e}"))?;
    let out = child.wait_with_output().map_err(|e| format!("gh: {e}"))?;
    if !out.status.success() {
        return Err(format!("gh secret set: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(())
}

fn sign_file(args: &[String]) -> Result<()> {
    let file = args.first().filter(|a| !a.starts_with("--")).map(PathBuf::from).ok_or("release-key sign FILE")?;
    let env = opt(args, "--env").unwrap_or_else(|| SECRET.into());
    let private = std::env::var(&env).ok().filter(|k| !k.trim().is_empty()).ok_or_else(|| format!("{env} isn't set: releases must be signed"))?;
    let data = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let sig = sign::sign(&private, &data)?;
    signing::verify(&data, sig.as_bytes()).map_err(|e| format!("{e}: {env} isn't one of the keys in {KEYS_FILE}"))?;
    let out = PathBuf::from(format!("{}.sig", file.display()));
    std::fs::write(&out, sig).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("{} signed ({})", file.display(), out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backup_is_private_and_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/release-key");
        write_backup(&path, "abcd").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "abcd\n");
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, PRIVATE);
        assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, PRIVATE_DIR);
        assert!(write_backup(&path, "other").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "abcd\n");
    }
}
