//! Mandara's update format, shared by the release tooling (patch creation)
//! and the updater in the shell (patch application).
//!
//! - **Patches** (`*.patch.zst`): a bsdiff-style delta from bidiff (suffix
//!   array matches, an "add" stream of byte differences that is mostly
//!   zeros, plus new bytes), compressed with zstd. Applying one needs the
//!   exact old file; the result is always checked against the published
//!   SHA-256 before it is used.
//! - **`update.json`**: the release manifest. One entry per target (shell
//!   CPU variants, or [`PLUGIN_TARGET`] for a plugin) with the full archive,
//!   the binary inside it, and the patches from earlier versions.
//!
//! The apply side is pure Rust (bipatch + ruzstd). The `create` feature adds
//! bidiff and the reference zstd encoder for CI.

use std::collections::BTreeMap;
use std::io::{self, Cursor, Read};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[cfg(feature = "archive")]
pub mod archive;
#[cfg(feature = "create")]
pub mod create;

/// File name of the release manifest.
pub const MANIFEST_FILE: &str = "update.json";
/// Manifest format version; readers reject newer schemas.
pub const SCHEMA: u32 = 1;
/// Suffix of a compressed patch.
pub const PATCH_SUFFIX: &str = ".patch.zst";
/// The target key plugin manifests use (plugins are WebAssembly).
pub const PLUGIN_TARGET: &str = "wasm32-wasip2";
/// The binary a plugin patch applies to.
pub const PLUGIN_BINARY: &str = "plugin.wasm";
/// The binary a shell patch applies to, inside the release tarball.
pub const SHELL_BINARY: &str = "bin/mandara";
/// Hex digits of a SHA-256.
pub const SHA256_HEX_LEN: usize = 64;

/// One release's update manifest (`update.json`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    /// `mandara`, or the plugin id.
    pub name: String,
    /// The release's version (no leading `v`).
    pub version: String,
    /// Target key (`x86_64-linux-gnu`, `x86_64-linux-gnu-x86-64-v3`,
    /// `aarch64-linux-gnu`, [`PLUGIN_TARGET`]) → its files.
    #[serde(default)]
    pub targets: BTreeMap<String, Target>,
}

/// One target's files in a release.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Target {
    /// The full archive (the fallback download).
    pub archive: FileRef,
    /// The binary inside the archive that patches rebuild.
    pub binary: Binary,
    /// Patches to this release, by the version they start from.
    #[serde(default)]
    pub patches: Vec<Patch>,
    /// Plugins: the archive without the binary (manifest, icons, locales),
    /// downloaded alongside a patch so every other file is current too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<FileRef>,
}

/// A release asset.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRef {
    /// Asset name in the release.
    pub file: String,
    pub sha256: String,
    pub size: u64,
}

/// The binary a patch produces, inside the archive.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binary {
    /// Path inside the archive, below its top folder (`bin/mandara`,
    /// `plugin.wasm`).
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

/// A delta from an earlier version's binary to this release's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Patch {
    /// The version it applies to.
    pub from: String,
    /// SHA-256 of that version's binary: the installed file must match.
    pub from_sha256: String,
    /// Asset name of the patch.
    pub file: String,
    pub sha256: String,
    pub size: u64,
}

impl Manifest {
    /// Parses `update.json`, rejecting newer schemas and malformed hashes.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let m: Manifest = serde_json::from_slice(bytes).map_err(|e| format!("update.json: {e}"))?;
        if m.schema == 0 || m.schema > SCHEMA {
            return Err(format!("update.json: unsupported schema {}", m.schema));
        }
        let hashes = m.targets.values().flat_map(|t| {
            let mut h = vec![&t.archive.sha256, &t.binary.sha256];
            h.extend(t.patches.iter().flat_map(|p| [&p.sha256, &p.from_sha256]));
            h.extend(t.meta.iter().map(|f| &f.sha256));
            h
        });
        for h in hashes {
            if !is_sha256(h) {
                return Err(format!("update.json: malformed SHA-256 {h:?}"));
            }
        }
        Ok(m)
    }

    /// The patch for `target` from `version` (a leading `v` is ignored).
    pub fn patch_for(&self, target: &str, version: &str) -> Option<&Patch> {
        let from = version.trim_start_matches('v');
        self.targets.get(target)?.patches.iter().find(|p| p.from.trim_start_matches('v') == from)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default() + "\n"
    }
}

/// 64 lowercase-or-uppercase hex digits.
pub fn is_sha256(s: &str) -> bool {
    s.len() == SHA256_HEX_LEN && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// SHA-256 as lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The hash of `file` in a `sha256sum` listing (`SHA256SUMS`).
pub fn sha256sums_lookup(listing: &str, file: &str) -> Option<String> {
    listing.lines().find_map(|line| {
        let (hash, name) = line.split_once(char::is_whitespace)?;
        let name = name.trim_start().trim_start_matches('*');
        (name == file && is_sha256(hash)).then(|| hash.to_ascii_lowercase())
    })
}

/// `mandara-0.1.0-to-0.1.1-x86_64-linux-gnu.patch.zst`, or for a plugin
/// (no target) `moon-phase-0.1.0-to-0.2.0.patch.zst`.
pub fn patch_name(name: &str, from: &str, to: &str, target: Option<&str>) -> String {
    let (from, to) = (from.trim_start_matches('v'), to.trim_start_matches('v'));
    match target {
        Some(t) => format!("{name}-{from}-to-{to}-{t}{PATCH_SUFFIX}"),
        None => format!("{name}-{from}-to-{to}{PATCH_SUFFIX}"),
    }
}

/// Rebuilds the new file from `old` and a compressed patch. Output beyond
/// `max_len` bytes is an error (a corrupt or hostile patch can't fill the
/// disk); the caller still verifies the result's hash.
pub fn apply(old: &[u8], patch: &[u8], max_len: u64) -> io::Result<Vec<u8>> {
    let decoder = ruzstd::decoding::StreamingDecoder::new(Cursor::new(patch)).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("patch: {e}")))?;
    let reader = bipatch::Reader::new(decoder, Cursor::new(old)).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("patch: {e}")))?;
    let mut out = Vec::new();
    reader.take(max_len.saturating_add(1)).read_to_end(&mut out)?;
    if out.len() as u64 > max_len {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "patch output exceeds the expected size"));
    }
    Ok(out)
}

/// [`apply`] that also checks the result against `expected_sha256`.
pub fn apply_verified(old: &[u8], patch: &[u8], expected_size: u64, expected_sha256: &str) -> Result<Vec<u8>, String> {
    let out = apply(old, patch, expected_size).map_err(|e| e.to_string())?;
    let got = sha256_hex(&out);
    if out.len() as u64 != expected_size || !got.eq_ignore_ascii_case(expected_sha256) {
        return Err(format!("patched file doesn't match the release (sha256 {got})"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        let h = "a".repeat(SHA256_HEX_LEN);
        let mut m = Manifest { schema: SCHEMA, name: "mandara".into(), version: "0.2.0".into(), targets: BTreeMap::new() };
        m.targets.insert(
            "x86_64-linux-gnu".into(),
            Target {
                archive: FileRef { file: "mandara-0.2.0-x86_64-linux-gnu.tar.gz".into(), sha256: h.clone(), size: 10 },
                binary: Binary { path: SHELL_BINARY.into(), sha256: h.clone(), size: 20 },
                patches: vec![Patch { from: "0.1.9".into(), from_sha256: h.clone(), file: patch_name("mandara", "0.1.9", "0.2.0", Some("x86_64-linux-gnu")), sha256: h, size: 3 }],
                meta: None,
            },
        );
        m
    }

    #[test]
    fn manifest_round_trips_and_finds_patches() {
        let m = manifest();
        let parsed = Manifest::parse(m.to_json().as_bytes()).unwrap();
        assert_eq!(parsed, m);
        assert_eq!(parsed.patch_for("x86_64-linux-gnu", "v0.1.9").unwrap().file, "mandara-0.1.9-to-0.2.0-x86_64-linux-gnu.patch.zst");
        assert!(parsed.patch_for("x86_64-linux-gnu", "0.1.8").is_none());
        assert!(parsed.patch_for("aarch64-linux-gnu", "0.1.9").is_none());
    }

    #[test]
    fn manifest_rejects_new_schemas_and_bad_hashes() {
        let mut m = manifest();
        m.schema = SCHEMA + 1;
        assert!(Manifest::parse(m.to_json().as_bytes()).is_err());
        let mut m = manifest();
        m.targets.get_mut("x86_64-linux-gnu").unwrap().binary.sha256 = "nothex".into();
        assert!(Manifest::parse(m.to_json().as_bytes()).is_err());
        assert!(Manifest::parse(b"{not json").is_err());
    }

    #[test]
    fn sha256sums_lookup_finds_exact_names() {
        let a = "0".repeat(SHA256_HEX_LEN);
        let b = "F".repeat(SHA256_HEX_LEN);
        let listing = format!("{a}  mandara-0.2.0-x86_64-linux-gnu.tar.gz\n{b} *mandara-0.2.0-x86_64-linux-gnu-x86-64-v3.tar.gz\n");
        assert_eq!(sha256sums_lookup(&listing, "mandara-0.2.0-x86_64-linux-gnu.tar.gz"), Some(a));
        assert_eq!(sha256sums_lookup(&listing, "mandara-0.2.0-x86_64-linux-gnu-x86-64-v3.tar.gz"), Some(b.to_ascii_lowercase()));
        assert_eq!(sha256sums_lookup(&listing, "mandara-0.2.0-aarch64-linux-gnu.tar.gz"), None);
    }

    #[test]
    fn garbage_is_not_a_patch() {
        assert!(apply(b"old", b"definitely not zstd", 100).is_err());
    }
}
