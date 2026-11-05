//! Complete cache files are published only after their encoder finishes.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

struct Pending(PathBuf);

impl Drop for Pending {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Each encoder owns its temporary file, including when two outputs request
/// the same wallpaper at once. Readers see either complete version.
pub(super) fn write(out: &Path, encode: impl FnOnce(&mut BufWriter<File>) -> Result<(), String>) -> Result<(), String> {
    let dir = out.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let name = out.file_name().ok_or("cache path has no file name")?.to_string_lossy();
    let (pending, file) = loop {
        let id = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(".{name}.{}.{id}.tmp", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => break (Pending(path), file),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err.to_string()),
        }
    };
    let mut writer = BufWriter::new(file);
    encode(&mut writer)?;
    writer.flush().map_err(|e| e.to_string())?;
    std::fs::rename(&pending.0, out).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_encoders_publish_complete_files() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("wallpaper.jpg");
        std::fs::write(&out, b"previous").unwrap();
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let out = &out;
            let barrier = &barrier;
            let jobs: Vec<_> = b"ab".iter().copied().map(|byte| scope.spawn(move || {
                write(out, |writer| {
                    writer.write_all(&[byte; 32]).map_err(|e| e.to_string())?;
                    assert_eq!(std::fs::read(out).unwrap(), b"previous");
                    barrier.wait();
                    Ok(())
                }).unwrap();
            })).collect();
            for job in jobs { job.join().unwrap(); }
        });
        let bytes = std::fs::read(&out).unwrap();
        assert!(bytes == [b'a'; 32] || bytes == [b'b'; 32]);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn failed_encoder_preserves_previous_file_and_removes_its_temporary() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("wallpaper.png");
        std::fs::write(&out, b"previous").unwrap();
        assert!(write(&out, |writer| {
            writer.write_all(b"partial").map_err(|e| e.to_string())?;
            Err("encoder failed".into())
        }).is_err());
        assert_eq!(std::fs::read(&out).unwrap(), b"previous");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn outputs_can_encode_the_same_jpeg_concurrently() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("wallpapers/same-size.jpg");
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(512, 512, |x, y| image::Rgb([x as u8, y as u8, (x ^ y) as u8])));
        let barrier = std::sync::Barrier::new(8);
        std::thread::scope(|scope| {
            let jobs: Vec<_> = (0..8).map(|_| scope.spawn(|| {
                barrier.wait();
                super::super::save_jpeg(&image, &out, super::super::DISPLAY_QUALITY).unwrap();
            })).collect();
            for job in jobs { job.join().unwrap(); }
        });
        let decoded = image::open(&out).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (512, 512));
        assert_eq!(std::fs::read_dir(out.parent().unwrap()).unwrap().count(), 1);
    }
}
