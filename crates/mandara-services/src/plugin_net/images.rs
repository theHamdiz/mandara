//! Content-addressed, bounded files for the host's existing thumbnail worker.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use image::{ImageFormat, ImageReader};
use sha2::{Digest as _, Sha256};

/// Remote thumbnails may not exceed this side before the shared worker decodes them.
const MAX_SIDE: u32 = 2048;
/// Encoded files shared by shell and Studio; decoded images remain view-owned.
const MAX_CACHE_BYTES: u64 = 16 * 1024 * 1024;
/// A small thumbnail must not let the on-disk cache grow without a file bound.
const MAX_CACHE_FILES: usize = 64;
/// Refuse a cache altered into a huge directory instead of retaining an unbounded scan.
const MAX_CACHE_SCAN: usize = 4096;

/// Validates headers and publishes a complete immutable image off the UI thread.
pub fn store(bytes: &[u8]) -> Result<PathBuf, String> {
    let root = dirs::cache_dir().ok_or("there is no cache folder")?.join("mandara/plugin-images");
    store_in(&root, bytes)
}

fn extension(bytes: &[u8]) -> Result<&'static str, String> {
    if bytes.len() > super::MAX_IMAGE_BYTES {
        return Err("the image response is too large".into());
    }
    let mut reader = ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    let extension = match reader.format() {
        Some(ImageFormat::Png) => "png",
        Some(ImageFormat::Jpeg) => "jpg",
        Some(ImageFormat::WebP) => "webp",
        _ => return Err("only PNG, JPEG and WebP thumbnails are supported".into()),
    };
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(u64::from(MAX_SIDE).pow(2) * 4);
    reader.limits(limits);
    let (width, height) = reader.into_dimensions().map_err(|e| format!("invalid thumbnail: {e}"))?;
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err("the thumbnail dimensions are too large or empty".into());
    }
    Ok(extension)
}

fn store_in(root: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
    let extension = extension(bytes)?;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(root).map_err(|e| e.to_string())?;
    // Shell and Studio share this cache, so publication and pruning need a process-shared lock.
    let lock = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false)
        .mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(root.join(".cache.lock")).map_err(|e| e.to_string())?;
    lock.lock().map_err(|e| e.to_string())?;
    let path = root.join(format!("{:x}.{extension}", Sha256::digest(bytes)));
    let mut pending = tempfile::NamedTempFile::new_in(root).map_err(|e| e.to_string())?;
    pending.write_all(bytes).map_err(|e| e.to_string())?;
    pending.persist(&path).map_err(|e| e.to_string())?;
    if let Err(error) = prune(root, &path) {
        return match std::fs::remove_file(&path) {
            Ok(()) => Err(error),
            Err(cleanup) => Err(format!("{error}; removing the unused thumbnail failed: {cleanup}")),
        };
    }
    Ok(path)
}

fn cache_name(name: &str) -> bool {
    let Some((digest, extension)) = name.split_once('.') else { return false };
    digest.len() == 64 && digest.bytes().all(|c| c.is_ascii_hexdigit()) && matches!(extension, "png" | "jpg" | "webp")
}

fn prune(root: &Path, keep: &Path) -> Result<(), String> {
    let mut files = Vec::new();
    let mut bytes = 0_u64;
    for (at, entry) in std::fs::read_dir(root).map_err(|e| e.to_string())?.enumerate() {
        if at >= MAX_CACHE_SCAN { return Err("the thumbnail cache contains too many entries".into()); }
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_name().to_str().is_some_and(cache_name) { continue; }
        let metadata = entry.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file() { continue; }
        bytes = bytes.saturating_add(metadata.len());
        files.push((entry.path(), metadata.len(), metadata.modified().unwrap_or(std::time::UNIX_EPOCH)));
    }
    files.sort_unstable_by_key(|(_, _, modified)| *modified);
    let mut count = files.len();
    for (path, size, _) in files {
        if count <= MAX_CACHE_FILES && bytes <= MAX_CACHE_BYTES { break; }
        if path == keep { continue; }
        std::fs::remove_file(path).map_err(|e| e.to_string())?;
        count -= 1;
        bytes -= size;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([10, 20, 30, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn headers_are_checked_before_a_thumbnail_is_issued() {
        assert_eq!(extension(&png(2, 3)).unwrap(), "png");
        assert!(extension(b"<svg>not a supported picture</svg>").is_err());
        assert!(extension(b"\x89PNG\r\n\x1a\n").is_err());
        assert!(extension(&png(MAX_SIDE + 1, 1)).is_err());
        assert!(extension(&vec![0; super::super::MAX_IMAGE_BYTES + 1]).is_err());
    }

    #[test]
    fn identical_images_share_a_complete_file_and_cache_pruning_preserves_unrelated_files() {
        let root = tempfile::tempdir().unwrap();
        let bytes = png(2, 3);
        let path = store_in(root.path(), &bytes).unwrap();
        assert_eq!(store_in(root.path(), &bytes).unwrap(), path);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let unrelated = root.path().join("owner-note");
        std::fs::write(&unrelated, b"keep").unwrap();
        for i in 0..MAX_CACHE_FILES {
            std::fs::write(root.path().join(format!("{i:064x}.png")), b"old").unwrap();
        }
        let path = store_in(root.path(), &bytes).unwrap();
        let count = std::fs::read_dir(root.path()).unwrap().flatten().filter(|e| e.file_name().to_str().is_some_and(cache_name)).count();
        assert_eq!(count, MAX_CACHE_FILES);
        assert!(path.exists());
        assert_eq!(std::fs::read(unrelated).unwrap(), b"keep");
    }

    #[test]
    fn byte_budget_evicts_old_files_even_below_the_file_count() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join(format!("{:064x}.png", 1));
        std::fs::File::create(&old).unwrap().set_len(MAX_CACHE_BYTES).unwrap();
        let path = store_in(root.path(), &png(1, 1)).unwrap();
        assert!(!old.exists());
        assert!(path.exists());
    }

    #[test]
    fn a_corrupted_oversized_cache_does_not_publish_an_untracked_image() {
        let root = tempfile::tempdir().unwrap();
        for at in 0..=MAX_CACHE_SCAN { std::fs::write(root.path().join(format!("owner-{at}")), b"").unwrap(); }
        assert!(store_in(root.path(), &png(1, 1)).is_err());
        assert!(!std::fs::read_dir(root.path()).unwrap().flatten().any(|entry| entry.file_name().to_str().is_some_and(cache_name)));
    }

    #[test]
    fn simultaneous_downloads_publish_complete_images_without_pruning_each_others_files() {
        let root = tempfile::tempdir().unwrap();
        let bytes = png(2, 3);
        let barrier = std::sync::Barrier::new(8);
        std::thread::scope(|scope| {
            let workers = (0..8).map(|_| scope.spawn(|| {
                barrier.wait();
                for _ in 0..4 {
                    let path = store_in(root.path(), &bytes).unwrap();
                    assert_eq!(std::fs::read(path).unwrap(), bytes);
                }
            })).collect::<Vec<_>>();
            for worker in workers { worker.join().unwrap(); }
        });
    }
}
