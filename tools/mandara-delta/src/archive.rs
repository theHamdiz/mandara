//! Reading release archives (`.tar.gz`): the binary a patch rebuilds.

use std::io::{self, Read};
use std::path::{Component, Path};

/// Whether an archive entry is `wanted` (`bin/mandara`), either at the root
/// or inside one top folder (`mandara-0.2.0-x86_64-linux-gnu/bin/mandara`).
pub fn entry_matches(entry: &Path, wanted: &str) -> bool {
    let parts: Vec<&std::ffi::OsStr> = entry.components().filter_map(|c| if let Component::Normal(n) = c { Some(n) } else { None }).collect();
    if entry.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) {
        return false;
    }
    let want: Vec<&str> = wanted.split('/').filter(|s| !s.is_empty()).collect();
    let tail = |skip: usize| parts.len() == want.len() + skip && parts[skip..].iter().zip(&want).all(|(a, b)| a.to_str() == Some(*b));
    tail(0) || tail(1)
}

/// The regular file `wanted` inside a gzip'd tarball, read up to `max`
/// bytes (larger entries are an error). `None` when absent.
pub fn extract_file(tar_gz: impl Read, wanted: &str, max: u64) -> io::Result<Option<Vec<u8>>> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(tar_gz));
    for entry in archive.entries()? {
        let entry = entry?;
        if !entry.header().entry_type().is_file() || !entry_matches(&entry.path()?, wanted) {
            continue;
        }
        if entry.header().size()? > max {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("{wanted} in the archive is larger than {max} bytes")));
        }
        let mut out = Vec::new();
        entry.take(max).read_to_end(&mut out)?;
        return Ok(Some(out));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_root_or_one_top_folder_only() {
        assert!(entry_matches(Path::new("bin/mandara"), "bin/mandara"));
        assert!(entry_matches(Path::new("mandara-0.2.0-x86_64-linux-gnu/bin/mandara"), "bin/mandara"));
        assert!(!entry_matches(Path::new("a/b/bin/mandara"), "bin/mandara"));
        assert!(!entry_matches(Path::new("../bin/mandara"), "bin/mandara"));
        assert!(!entry_matches(Path::new("top/bin/mandara.old"), "bin/mandara"));
        assert!(entry_matches(Path::new("moon-phase/plugin.wasm"), "plugin.wasm"));
    }
}
