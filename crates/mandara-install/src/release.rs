//! GitHub releases: where a version's assets live, and checking a download
//! or this very binary against the release's published SHA-256.

use std::path::{Path, PathBuf};

use mandara_delta::{MANIFEST_FILE, Manifest, SHELL_BINARY};

/// The shell's GitHub repository.
pub const DEFAULT_REPO: &str = "theHamdiz/mandara";
/// Overrides the repository (as `--repo` does).
pub const REPO_ENV: &str = "MANDARA_REPO";
/// A mirror of one release's assets (needs `--version`).
pub const RELEASE_BASE_ENV: &str = "MANDARA_RELEASE_BASE";
/// The checksum listing every release carries.
pub const SUMS_FILE: &str = "SHA256SUMS";
pub const ARCHIVE_SUFFIX: &str = ".tar.gz";
/// Largest JSON / checksum answer read.
pub const MAX_JSON_BYTES: u64 = 4 << 20;
/// Largest release archive downloaded.
pub const MAX_ARCHIVE_BYTES: u64 = 512 << 20;
/// Largest file extracted from an archive.
pub const MAX_BINARY_BYTES: u64 = 1 << 30;
const GITHUB_API: &str = "https://api.github.com";
const GITHUB: &str = "https://github.com";

/// `releases/latest` of `repo` on GitHub's API.
pub fn latest_url(repo: &str) -> String {
    format!("{GITHUB_API}/repos/{repo}/releases/latest")
}

/// The version (`tag_name` without `v`) of a `releases/latest` answer.
pub fn parse_latest(json: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(json).ok()?;
    let tag = v.get("tag_name")?.as_str()?.trim().trim_start_matches('v');
    (!tag.is_empty()).then(|| tag.to_string())
}

/// Where `version`'s assets are downloaded from: the mirror when set, else
/// the GitHub release.
pub fn release_base(repo: &str, version: &str, mirror: Option<&str>) -> String {
    match mirror.map(str::trim).filter(|m| !m.is_empty()) {
        Some(m) => m.trim_end_matches('/').to_string(),
        None => format!("{GITHUB}/{repo}/releases/download/v{version}"),
    }
}

/// The source repository's clone URL.
pub fn clone_url(repo: &str) -> String {
    format!("{GITHUB}/{repo}.git")
}

/// Checks `bytes` (the asset `file`) against a `SHA256SUMS` listing.
pub fn verify_sums(bytes: &[u8], sums: &str, file: &str) -> Result<String, String> {
    let actual = mandara_delta::sha256_hex(bytes);
    match mandara_delta::sha256sums_lookup(sums, file) {
        Some(expected) if expected.eq_ignore_ascii_case(&actual) => Ok(actual),
        expected => Err(format!("checksum mismatch for {file} (expected {}, got {actual})", expected.as_deref().unwrap_or("none"))),
    }
}

/// A file inside a release archive (`bin/mandara`, below its top folder).
pub fn extract(archive: &[u8], path: &str) -> Result<Option<Vec<u8>>, String> {
    mandara_delta::archive::extract_file(archive, path, MAX_BINARY_BYTES).map_err(|e| format!("reading the archive: {e}"))
}

/// The outcome of checking this binary against its release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelfCheck {
    /// Matches the published hash; says what it was checked against.
    Verified(String),
    /// Differs from what was published; says how.
    Mismatch(String),
    /// Nothing published could be reached; says why.
    Unavailable(String),
}

/// The binary's published hash for `target` in an `update.json`.
pub fn manifest_binary_sha(update_json: &[u8], version: &str, target: &str) -> Result<Option<String>, String> {
    let m = Manifest::parse(update_json)?;
    if m.version.trim_start_matches('v') != version {
        return Err(format!("{MANIFEST_FILE} describes {} instead of {version}", m.version));
    }
    Ok(m.targets.get(target).map(|t| t.binary.sha256.to_ascii_lowercase()))
}

/// Release tarballs of `version`/`target` that may sit next to an
/// extracted binary: the archive's own folder's parent (`tar xf` in a
/// download folder), then `cwd`. Both the versioned name and the
/// version-less alias are looked for.
pub fn local_archives(exe: &Path, cwd: &Path, version: &str, target: &str) -> Vec<PathBuf> {
    let names = [format!("mandara-{version}-{target}{ARCHIVE_SUFFIX}"), format!("mandara-{target}{ARCHIVE_SUFFIX}")];
    // `<dir>/mandara-<v>-<target>/bin/mandara` → `<dir>`.
    let extracted_in = exe.parent().and_then(Path::parent).and_then(Path::parent);
    let mut out = Vec::new();
    for dir in extracted_in.into_iter().chain([cwd]) {
        for n in &names {
            let p = dir.join(n);
            if p.is_file() && !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// Checks `exe_bytes` against a local release tarball verified by
/// `SHA256SUMS`: the tarball's hash must be listed and its `bin/mandara`
/// must equal this binary.
pub fn check_against_archive(exe_bytes: &[u8], archive: &[u8], archive_name: &str, sums: &str) -> SelfCheck {
    if let Err(e) = verify_sums(archive, sums, archive_name) {
        return SelfCheck::Mismatch(e);
    }
    match extract(archive, SHELL_BINARY) {
        Ok(Some(bin)) if bin == exe_bytes => SelfCheck::Verified(format!("{archive_name} and {SUMS_FILE}")),
        Ok(Some(bin)) => SelfCheck::Mismatch(format!("{SHELL_BINARY} in {archive_name} is {}, this binary is {}", mandara_delta::sha256_hex(&bin), mandara_delta::sha256_hex(exe_bytes))),
        Ok(None) => SelfCheck::Unavailable(format!("{archive_name} has no {SHELL_BINARY}")),
        Err(e) => SelfCheck::Unavailable(e),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A gzip'd tarball with the given (path, contents) entries.
    pub(crate) fn tarball(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        for (path, data) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o755);
            h.set_cksum();
            tar.append_data(&mut h, path, *data).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn urls() {
        assert_eq!(latest_url("a/b"), "https://api.github.com/repos/a/b/releases/latest");
        assert_eq!(release_base("a/b", "0.2.0", None), "https://github.com/a/b/releases/download/v0.2.0");
        assert_eq!(release_base("a/b", "0.2.0", Some("https://mirror.example/m/")), "https://mirror.example/m");
        assert_eq!(release_base("a/b", "0.2.0", Some(" ")), "https://github.com/a/b/releases/download/v0.2.0");
        assert_eq!(clone_url("a/b"), "https://github.com/a/b.git");
        assert_eq!(parse_latest(br#"{"tag_name":"v0.3.0","name":"x"}"#).as_deref(), Some("0.3.0"));
        assert_eq!(parse_latest(br#"{"message":"Not Found"}"#), None);
        assert_eq!(parse_latest(b"garbage"), None);
    }

    #[test]
    fn sha256sums_verification() {
        let data = b"tarball bytes";
        let sum = mandara_delta::sha256_hex(data);
        let sums = format!("{sum}  mandara-0.2.0-x86_64-linux-gnu.tar.gz\n{}  other\n", "0".repeat(64));
        assert_eq!(verify_sums(data, &sums, "mandara-0.2.0-x86_64-linux-gnu.tar.gz").unwrap(), sum);
        let err = verify_sums(b"tampered", &sums, "mandara-0.2.0-x86_64-linux-gnu.tar.gz").unwrap_err();
        assert!(err.contains("checksum mismatch") && err.contains(&sum), "{err}");
        assert!(verify_sums(data, &sums, "missing.tar.gz").unwrap_err().contains("expected none"));
    }

    #[test]
    fn checks_this_binary_against_update_json() {
        let sha = mandara_delta::sha256_hex(b"bin");
        let json = format!(r#"{{"schema":1,"name":"mandara","version":"0.2.0","targets":{{"x86_64-linux-gnu":{{"archive":{{"file":"a.tar.gz","sha256":"{sha}","size":1}},"binary":{{"path":"bin/mandara","sha256":"{sha}","size":3}}}}}}}}"#);
        assert_eq!(manifest_binary_sha(json.as_bytes(), "0.2.0", "x86_64-linux-gnu").unwrap(), Some(sha));
        assert_eq!(manifest_binary_sha(json.as_bytes(), "0.2.0", "aarch64-linux-gnu").unwrap(), None);
        assert!(manifest_binary_sha(json.as_bytes(), "0.3.0", "x86_64-linux-gnu").is_err());
        assert!(manifest_binary_sha(b"{}", "0.2.0", "x").is_err());
    }

    #[test]
    fn checks_this_binary_against_a_local_tarball() {
        let exe = b"\x7fELF mandara".to_vec();
        let name = "mandara-0.2.0-x86_64-linux-gnu.tar.gz";
        let archive = tarball(&[("mandara-0.2.0-x86_64-linux-gnu/bin/mandara", &exe)]);
        let sums = format!("{}  {name}\n", mandara_delta::sha256_hex(&archive));
        assert!(matches!(check_against_archive(&exe, &archive, name, &sums), SelfCheck::Verified(_)));
        assert!(matches!(check_against_archive(b"other", &archive, name, &sums), SelfCheck::Mismatch(_)));
        assert!(matches!(check_against_archive(&exe, &archive, name, "nothing"), SelfCheck::Mismatch(_)));
        let empty = tarball(&[("README.md", b"x")]);
        let sums = format!("{}  {name}\n", mandara_delta::sha256_hex(&empty));
        assert!(matches!(check_against_archive(&exe, &empty, name, &sums), SelfCheck::Unavailable(_)));
    }

    #[test]
    fn finds_tarballs_next_to_the_extracted_folder() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("mandara-0.2.0-x86_64-linux-gnu/bin/mandara");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(dir.path().join("mandara-0.2.0-x86_64-linux-gnu.tar.gz"), b"").unwrap();
        let cwd = tempfile::tempdir().unwrap();
        std::fs::write(cwd.path().join("mandara-x86_64-linux-gnu.tar.gz"), b"").unwrap();
        let found = local_archives(&exe, cwd.path(), "0.2.0", "x86_64-linux-gnu");
        assert_eq!(found, [dir.path().join("mandara-0.2.0-x86_64-linux-gnu.tar.gz"), cwd.path().join("mandara-x86_64-linux-gnu.tar.gz")]);
    }
}
