//! Hash owned files without retaining their contents during palette changes.

use super::write_lock::FileStamp;

use std::{
    fs::File,
    io::{self, Read as _},
    path::Path,
};

/// FNV-1a's fixed offset and multiplier, shared by memory and file hashing.
const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;
/// One sequential reader runs on the theme worker; large user edits never
/// require a file-sized allocation just to compare an ownership fingerprint.
const CHUNK_BYTES: usize = 64 * 1024;
fn update(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
    })
}

/// 64-bit content identity, not a security boundary.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    update(OFFSET, bytes)
}

pub(super) fn matches(path: &Path, expected: &str) -> io::Result<bool> {
    let Ok(expected) = u64::from_str_radix(expected, 16) else {
        return Ok(false);
    };
    let Some(initial) = FileStamp::at(path)? else {
        return Ok(false);
    };
    let file = File::open(path)?;
    if FileStamp::of(&file)? != initial {
        return Ok(false);
    }
    // Finish at the captured length even if another process keeps appending.
    let length = file.metadata()?.len();
    let mut file = file.take(length);
    let mut hash = OFFSET;
    let mut buffer = [0; CHUNK_BYTES];
    loop {
        let count = match file.read(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            break;
        }
        hash = update(hash, &buffer[..count]);
    }
    Ok(file.limit() == 0 && hash == expected && FileStamp::at(path)? == Some(initial))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_and_non_regular_paths_are_never_owned_files() {
        let root = tempfile::tempdir().unwrap();
        assert!(matches(root.path(), "0000000000000000").is_err());
        assert!(!matches(&root.path().join("missing"), "0000000000000000").unwrap());
    }
    #[test]
    fn streamed_fingerprints_match_existing_ownership_records_across_chunks() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("theme");
        let bytes: Vec<_> = (0..CHUNK_BYTES * 3 + 7).map(|index| index as u8).collect();
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches(&path, &format!("{:016x}", fnv1a(&bytes))).unwrap());
        assert!(!matches(&path, "0000000000000000").unwrap());
        assert!(!matches(&path, "broken record").unwrap());
    }
}
