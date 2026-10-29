//! Patch and manifest creation (release tooling; the `create` feature).

use std::io::{self, Cursor, Read};
use std::path::Path;

use crate::{Binary, FileRef, Manifest, PLUGIN_BINARY, PLUGIN_TARGET, Patch, SCHEMA, Target, apply_verified, patch_name, sha256_hex};

/// zstd level for patches: bsdiff's add stream is mostly zero runs, and a
/// patch is compressed once and downloaded many times, so CI pays for the
/// strongest regular level.
pub const ZSTD_LEVEL: i32 = 19;
/// Largest file in a plugin archive read into memory.
const MAX_PLUGIN_FILE: u64 = 64 << 20;
/// Mode of files in a plugin meta archive.
const MODE_FILE: u32 = 0o644;

/// A compressed patch turning `old` into `new`.
pub fn diff(old: &[u8], new: &[u8]) -> io::Result<Vec<u8>> {
    let mut raw = Vec::with_capacity(new.len() / 4);
    bidiff::simple_diff(old, new, &mut raw)?;
    zstd::stream::encode_all(Cursor::new(raw), ZSTD_LEVEL)
}

/// [`diff`], then applies the patch with the updater's own (pure-Rust)
/// decoder and checks it rebuilds `new` byte for byte.
pub fn diff_verified(old: &[u8], new: &[u8]) -> Result<Vec<u8>, String> {
    let patch = diff(old, new).map_err(|e| format!("creating the patch: {e}"))?;
    apply_verified(old, &patch, new.len() as u64, &sha256_hex(new)).map_err(|e| format!("verifying the patch: {e}"))?;
    Ok(patch)
}

/// A release file: its asset name and bytes.
pub type Asset = (String, Vec<u8>);

/// One target's manifest entry for a release. With `previous` (its version
/// and binary), also the patch from it (verified), returned as an asset.
pub fn target_entry(name: &str, version: &str, target: Option<&str>, archive: &Asset, binary_path: &str, new_bin: &[u8], previous: Option<(&str, &[u8])>) -> Result<(Target, Option<Asset>), String> {
    let mut entry = Target {
        archive: FileRef { file: archive.0.clone(), sha256: sha256_hex(&archive.1), size: archive.1.len() as u64 },
        binary: Binary { path: binary_path.to_string(), sha256: sha256_hex(new_bin), size: new_bin.len() as u64 },
        patches: Vec::new(),
        meta: None,
    };
    let mut patch_asset = None;
    if let Some((from, old_bin)) = previous {
        let patch = diff_verified(old_bin, new_bin)?;
        let file = patch_name(name, from, version, target);
        entry.patches.push(Patch { from: from.trim_start_matches('v').to_string(), from_sha256: sha256_hex(old_bin), file: file.clone(), sha256: sha256_hex(&patch), size: patch.len() as u64 });
        patch_asset = Some((file, patch));
    }
    Ok((entry, patch_asset))
}

/// A plugin archive's files (paths relative to its plugin.toml) with the
/// manifest's id and version.
pub struct PluginArchive {
    pub id: String,
    pub version: String,
    pub files: Vec<(String, Vec<u8>)>,
}

impl PluginArchive {
    pub fn read(tar_gz: &[u8]) -> Result<Self, String> {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(tar_gz));
        let mut files = Vec::new();
        for entry in archive.entries().map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if !entry.header().entry_type().is_file() {
                continue;
            }
            let path = entry.path().map_err(|e| e.to_string())?.to_string_lossy().trim_start_matches("./").to_string();
            let mut data = Vec::new();
            entry.take(MAX_PLUGIN_FILE).read_to_end(&mut data).map_err(|e| e.to_string())?;
            files.push((path, data));
        }
        // plugin.toml at the root or inside one top folder.
        let prefix = if files.iter().any(|(p, _)| p == "plugin.toml") {
            String::new()
        } else {
            let tops: Vec<&str> = files.iter().filter_map(|(p, _)| p.strip_suffix("/plugin.toml")).filter(|t| !t.contains('/')).collect();
            match tops.as_slice() {
                [one] => format!("{one}/"),
                _ => return Err("the plugin archive has no plugin.toml".into()),
            }
        };
        let files: Vec<(String, Vec<u8>)> = files.into_iter().filter_map(|(p, d)| p.strip_prefix(&prefix).map(|r| (r.to_string(), d))).collect();
        let toml_text = files.iter().find(|(p, _)| p == "plugin.toml").map(|(_, d)| String::from_utf8_lossy(d).to_string()).unwrap_or_default();
        let table: toml::Table = toml::from_str(&toml_text).map_err(|e| format!("plugin.toml: {e}"))?;
        let field = |k: &str| table.get(k).and_then(|v| v.as_str()).map(str::to_string).ok_or_else(|| format!("plugin.toml has no {k}"));
        Ok(Self { id: field("id")?, version: field("version")?, files })
    }

    fn file(&self, path: &str) -> Option<&[u8]> {
        self.files.iter().find(|(p, _)| p == path).map(|(_, d)| d.as_slice())
    }

    /// Everything but the binary, as `<id>/…` in a reproducible tarball.
    fn meta_archive(&self) -> Result<Vec<u8>, String> {
        let mut files: Vec<&(String, Vec<u8>)> = self.files.iter().filter(|(p, _)| p != PLUGIN_BINARY).collect();
        files.sort_by(|a, b| a.0.cmp(&b.0));
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best()));
        for (path, data) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(MODE_FILE);
            h.set_mtime(0);
            h.set_cksum();
            tar.append_data(&mut h, Path::new(&self.id).join(path), data.as_slice()).map_err(|e| e.to_string())?;
        }
        tar.into_inner().and_then(|gz| gz.finish()).map_err(|e| e.to_string())
    }
}

/// A plugin release's `update.json` and extra assets (the meta archive, and
/// the patch when the previous release's archive is given).
pub fn plugin_release(new_archive: &Asset, old_archive: Option<&[u8]>) -> Result<(Manifest, Vec<Asset>), String> {
    let new = PluginArchive::read(&new_archive.1)?;
    let new_wasm = new.file(PLUGIN_BINARY).ok_or("the plugin archive has no plugin.wasm")?;
    let old = old_archive.map(PluginArchive::read).transpose()?;
    if let Some(old) = &old
        && old.id != new.id
    {
        return Err(format!("plugin id changed ({} → {}); no patch possible", old.id, new.id));
    }
    let previous = old.as_ref().and_then(|o| o.file(PLUGIN_BINARY).map(|w| (o.version.as_str(), w)));
    let (mut entry, patch) = target_entry(&new.id, &new.version, None, new_archive, PLUGIN_BINARY, new_wasm, previous)?;
    let meta = new.meta_archive()?;
    let meta_name = format!("{}-{}.meta.tar.gz", new.id, new.version.trim_start_matches('v'));
    entry.meta = Some(FileRef { file: meta_name.clone(), sha256: sha256_hex(&meta), size: meta.len() as u64 });
    let mut manifest = Manifest { schema: SCHEMA, name: new.id.clone(), version: new.version.trim_start_matches('v').to_string(), ..Default::default() };
    manifest.targets.insert(PLUGIN_TARGET.to_string(), entry);
    let mut assets = vec![(meta_name, meta)];
    assets.extend(patch);
    Ok((manifest, assets))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random bytes (xorshift), so tests need no RNG.
    fn noise(len: usize, mut seed: u64) -> Vec<u8> {
        (0..len)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed as u8
            })
            .collect()
    }

    #[test]
    fn patches_rebuild_the_new_file_with_the_pure_rust_decoder() {
        let old = noise(1 << 20, 7);
        let mut new = old.clone();
        new.splice(4096..4096, noise(3000, 9));
        new.drain(500_000..520_000);
        for b in &mut new[700_000..700_100] {
            *b = b.wrapping_add(1);
        }
        let patch = diff_verified(&old, &new).unwrap();
        assert!(patch.len() < new.len() / 50, "patch of {} bytes for a {} byte file", patch.len(), new.len());
        assert_eq!(crate::apply(&old, &patch, new.len() as u64).unwrap(), new);
    }

    fn plugin_tar(version: &str, wasm: &[u8]) -> Vec<u8> {
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        let toml = format!("id = \"demo\"\nname = \"Demo\"\nversion = \"{version}\"\n");
        for (p, d) in [("demo/plugin.toml", toml.as_bytes()), ("demo/plugin.wasm", wasm), ("demo/locales/ar.toml", b"x = \"y\"\n".as_slice())] {
            let mut h = tar::Header::new_gnu();
            h.set_size(d.len() as u64);
            h.set_mode(MODE_FILE);
            h.set_cksum();
            tar.append_data(&mut h, p, d).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn plugin_release_has_meta_patch_and_manifest() {
        let old_wasm = noise(200_000, 3);
        let mut new_wasm = old_wasm.clone();
        new_wasm.extend(noise(1000, 4));
        let old = plugin_tar("0.1.0", &old_wasm);
        let new = ("demo.mandara.tar.gz".to_string(), plugin_tar("0.2.0", &new_wasm));
        let (m, assets) = plugin_release(&new, Some(&old)).unwrap();
        assert_eq!((m.name.as_str(), m.version.as_str()), ("demo", "0.2.0"));
        let t = &m.targets[PLUGIN_TARGET];
        assert_eq!(t.binary.sha256, sha256_hex(&new_wasm));
        let patch = m.patch_for(PLUGIN_TARGET, "0.1.0").unwrap();
        assert_eq!(patch.file, "demo-0.1.0-to-0.2.0.patch.zst");
        let patch_bytes = &assets.iter().find(|(n, _)| *n == patch.file).unwrap().1;
        assert_eq!(crate::apply(&old_wasm, patch_bytes, t.binary.size).unwrap(), new_wasm);
        let meta = &assets.iter().find(|(n, _)| Some(n) == t.meta.as_ref().map(|f| &f.file)).unwrap().1;
        let meta = PluginArchive::read(meta).unwrap();
        assert!(meta.file(PLUGIN_BINARY).is_none() && meta.file("locales/ar.toml").is_some());
        assert_eq!(meta.version, "0.2.0");
        // Without a previous archive: manifest and meta only.
        let (m, assets) = plugin_release(&new, None).unwrap();
        assert!(m.targets[PLUGIN_TARGET].patches.is_empty() && assets.len() == 1);
    }
}
