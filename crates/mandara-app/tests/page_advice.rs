//! Guards against reclaiming mappings whose lifetime the shell does not own.
//!
//! A procfs snapshot cannot keep a temporary font or driver mapping alive.
//! Reusing one of those addresses before MADV_DONTNEED could erase unrelated
//! anonymous data. File-backed reclaim belongs to the kernel; freed heap pages
//! use the registered allocator trim hook instead. This source guard detects
//! the listed literal call spellings in mandara-app/src; it is not an alias or
//! dependency analysis. The guard itself lives outside the scanned sources.

#[test]
fn app_sources_do_not_use_the_listed_raw_page_advice_calls() {
    fn inspect(directory: &std::path::Path) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                inspect(&path);
            } else if path.extension().is_some_and(|extension| extension == "rs")
            {
                let source = std::fs::read_to_string(&path).unwrap();
                for forbidden in ["libc::madvise", "libc::posix_fadvise", "libc::msync"] {
                    assert!(!source.contains(forbidden), "{}: unowned page advice", path.display());
                }
            }
        }
    }
    inspect(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"));
}
