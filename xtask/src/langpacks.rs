//! Language packs (`mandara_i18n::pack`): shell and greeter catalogs
//! compiled into one `.mlang` file per language, mapped at run time.
//!
//! `langpacks [--version V | --bin PATH] [--dir DIR] [--out DIR]`:
//!
//! - `--dir DIR` writes `DIR/<code>.mlang` (the layout of
//!   `share/mandara/locales/<v>/`: tarballs, packages, the sandbox);
//! - `--out DIR` writes the release assets: `mandara-<v>-<code>.mlang`, its
//!   zstd-compressed download `….mlang.zst`, and
//!   `mandara-<v>-locales.update-part.json`, the `locales` part of
//!   `update.json` (the publish job merges the parts).
//!
//! The version is the binary's (`--bin`, read from `mandara --version`),
//! else `--version`, else the shell's recorded version. Packs are
//! architecture-independent and byte-for-byte reproducible.

use std::path::{Path, PathBuf};

use mandara_delta::{LocalePack, Manifest, SCHEMA};
use mandara_i18n::Lang;

use crate::{Result, opt, root, state};

/// Suffix of a manifest part (the same as the targets' parts).
const PART_SUFFIX: &str = ".update-part.json";

/// The shell's version as `bin --version` reports it (`mandara 0.10.2 (…)`).
pub fn binary_version(bin: &Path) -> Option<String> {
    let out = std::process::Command::new(bin).arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let v = text.split_whitespace().nth(1)?.to_string();
    (out.status.success() && v.chars().next().is_some_and(|c| c.is_ascii_digit())).then_some(v)
}

/// The recorded version of the shell (`versions.toml`).
pub fn recorded_version() -> String {
    state().record.get("mandara-app").map(|r| r.version.clone()).unwrap_or_else(|| "0.0.0".into())
}

/// Every language's pack for `version`, from the source catalogs.
pub fn compile_all(version: &str) -> Result<Vec<(Lang, Vec<u8>)>> {
    let dir = root().join("crates/mandara-i18n/locales");
    Lang::ALL
        .into_iter()
        .filter(|l| *l != Lang::En)
        .map(|lang| {
            mandara_i18n::bundle::compile_files(lang, version, &dir, &root().join("crates/mandara-greeter/locales")).map(|bytes| (lang, bytes))
        })
        .collect()
}

/// Writes `DIR/<code>.mlang` for every language.
pub fn write_dir(dir: &Path, version: &str) -> Result<usize> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let packs = compile_all(version)?;
    for (lang, bytes) in &packs {
        let path = dir.join(mandara_i18n::packs::file_name(*lang));
        std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(packs.len())
}

pub fn langpacks(args: &[String]) -> Result<()> {
    let version = match opt(args, "--bin") {
        Some(bin) => binary_version(Path::new(&bin)).ok_or_else(|| format!("{bin} --version didn't report a version"))?,
        None => opt(args, "--version").unwrap_or_else(recorded_version),
    };
    let version = version.trim_start_matches('v').to_string();
    if let Some(dir) = opt(args, "--dir").map(PathBuf::from) {
        let n = write_dir(&dir, &version)?;
        println!("{n} language packs for {version} in {}", dir.display());
    }
    if let Some(out) = opt(args, "--out").map(|o| root().join(o)) {
        std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        let mut part = Manifest { schema: SCHEMA, name: "mandara".into(), version: version.clone(), ..Default::default() };
        let (mut raw, mut packed) = (0usize, 0usize);
        for (lang, bytes) in compile_all(&version)? {
            let name = mandara_delta::locale_name(&version, lang.code());
            let zst = mandara_delta::create::compress(&bytes).map_err(|e| format!("{name}: {e}"))?;
            // Checked with the client's own (pure-Rust) decoder.
            if mandara_delta::decompress(&zst, bytes.len() as u64).map_err(|e| format!("{name}: {e}"))? != bytes {
                return Err(format!("{name}: the compressed pack doesn't decompress to itself"));
            }
            let zst_name = format!("mandara-{version}-{}{}", lang.code(), mandara_delta::LOCALE_ZST_SUFFIX);
            std::fs::write(out.join(&name), &bytes).map_err(|e| format!("{name}: {e}"))?;
            std::fs::write(out.join(&zst_name), &zst).map_err(|e| format!("{zst_name}: {e}"))?;
            println!("{name}: {} bytes, {zst_name}: {} bytes", bytes.len(), zst.len());
            (raw, packed) = (raw + bytes.len(), packed + zst.len());
            part.locales.insert(
                lang.code().to_string(),
                LocalePack { file: zst_name, sha256: mandara_delta::sha256_hex(&zst), size: zst.len() as u64, pack_sha256: mandara_delta::sha256_hex(&bytes), pack_size: bytes.len() as u64 },
            );
        }
        let path = out.join(format!("mandara-{version}-locales{PART_SUFFIX}"));
        std::fs::write(&path, part.to_json()).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("{} language packs: {raw} bytes, {packed} bytes to download; {}", part.locales.len(), path.display());
    }
    if opt(args, "--dir").is_none() && opt(args, "--out").is_none() {
        return Err("langpacks needs --dir DIR and/or --out DIR".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_compiles_reproducibly() {
        let a = compile_all("1.2.3").unwrap();
        assert_eq!(a.len(), Lang::ALL.len() - 1, "every language but English");
        assert_eq!(a, compile_all("1.2.3").unwrap(), "the same sources give the same bytes");
        for (lang, bytes) in &a {
            let pack = mandara_i18n::pack::Pack::parse(bytes, "1.2.3", *lang).unwrap();
            assert!(pack.len() > 1000, "{lang:?} has {} strings", pack.len());
            let greeter = std::fs::read_to_string(root().join("crates/mandara-greeter/locales").join(format!("{}.toml", lang.code()))).unwrap();
            let catalog = mandara_i18n::Catalog::parse(&greeter).unwrap();
            for (key, value) in catalog.iter() {
                assert_eq!(pack.get_prefixed(mandara_i18n::bundle::GREETER_PREFIX, key), Some(value), "{}: {key}", lang.code());
            }
        }
    }
}
