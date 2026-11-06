//! Bounded recitation downloads, streamed to an atomic disk cache.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use mandara_i18n::{t, tf};

use super::Observer;

/// Network bounds apply even when a provider omits Content-Length.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const BODY_TIMEOUT: Duration = Duration::from_secs(300);
/// Only this chunk, rather than a chapter, is held in memory.
const CHUNK: usize = 64 * 1024;
/// Progress notifications are coarser than reads to avoid flooding views.
const PROGRESS_BYTES: u64 = 256 * 1024;
pub const MIN_AUDIO: u64 = 1024;
pub const MAX_AYAH: u64 = 24 << 20;
pub const MAX_CHAPTER: u64 = 256 << 20;
pub const MAX_TIMING: u64 = 2 << 20;
static NEXT_PART: AtomicU64 = AtomicU64::new(0);

pub trait Transport {
    fn audio(&self, url: &str, path: &Path, limit: u64, observer: &dyn Observer) -> Result<(), String>;
    fn metadata(&self, url: &str, observer: &dyn Observer) -> Result<Vec<u8>, String>;
}

pub struct Http;

fn request(url: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    crate::offline::check()?;
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_connect(Some(CONNECT_TIMEOUT)).timeout_recv_body(Some(BODY_TIMEOUT)).build().into();
    agent.get(url).header("User-Agent", concat!("mandara-shell/", env!("CARGO_PKG_VERSION"))).call().map_err(|e| tf("Downloading {url}: {error}", &[("url", &url), ("error", &e)]))
}

impl Transport for Http {
    fn audio(&self, url: &str, path: &Path, limit: u64, observer: &dyn Observer) -> Result<(), String> {
        observer.check()?;
        let mut response = request(url)?;
        let total = response.headers().get("content-length").and_then(|n| n.to_str().ok()).and_then(|n| n.parse().ok()).unwrap_or(0);
        if total > limit {
            return Err(t("The download is larger than expected").into());
        }
        let mut reader = response.body_mut().with_config().limit(limit.saturating_add(1)).reader();
        save_audio(&mut reader, path, limit, total, observer)
    }

    fn metadata(&self, url: &str, observer: &dyn Observer) -> Result<Vec<u8>, String> {
        observer.check()?;
        let mut response = request(url)?;
        let mut reader = response.body_mut().with_config().limit(MAX_TIMING.saturating_add(1)).reader();
        read_metadata(&mut reader, observer)
    }
}

fn read_metadata(reader: &mut dyn Read, observer: &dyn Observer) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let mut buf = [0; CHUNK];
    loop {
        observer.check()?;
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        if bytes.len() as u64 + n as u64 > MAX_TIMING {
            return Err(t("The download is larger than expected").into());
        }
        bytes.extend_from_slice(&buf[..n]);
    }
    observer.check()?;
    Ok(bytes)
}

pub fn cached_audio(path: &Path, limit: u64) -> bool {
    std::fs::File::open(path).is_ok_and(|mut f| {
        let mut head = [0; 3];
        f.metadata().is_ok_and(|m| (MIN_AUDIO..=limit).contains(&m.len())) && f.read_exact(&mut head).is_ok() && super::super::player::looks_like_mp3(&head)
    })
}

fn save_audio(reader: &mut dyn Read, path: &Path, limit: u64, total: u64, observer: &dyn Observer) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| "no cache folder".to_string())?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let serial = NEXT_PART.fetch_add(1, Ordering::Relaxed);
    let part = path.with_extension(format!("mp3.{}.{serial}.part", std::process::id()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&part).map_err(|e| e.to_string())?;
        let mut buf = [0; CHUNK];
        let mut head = [0; 3];
        let mut head_len = 0;
        let mut done = 0;
        let mut notified = 0;
        observer.progress(0, total);
        loop {
            observer.check()?;
            let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            done += n as u64;
            if done > limit {
                return Err(t("The download is larger than expected").into());
            }
            let take = (head.len() - head_len).min(n);
            head[head_len..head_len + take].copy_from_slice(&buf[..take]);
            head_len += take;
            file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            if done - notified >= PROGRESS_BYTES {
                observer.progress(done, total.max(done));
                notified = done;
            }
        }
        observer.check()?;
        if done < MIN_AUDIO || (total != 0 && done != total) || !super::super::player::looks_like_mp3(&head[..head_len]) {
            return Err(t("The recitation download is incomplete or is not an MP3 file").into());
        }
        file.sync_all().map_err(|e| e.to_string())?;
        observer.check()?;
        std::fs::rename(&part, path).map_err(|e| e.to_string())?;
        observer.progress(done, done);
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct Watch {
        reads: Cell<usize>,
        cancel_at: usize,
    }
    impl Observer for Watch {
        fn current(&self) -> bool {
            let n = self.reads.get();
            self.reads.set(n + 1);
            n < self.cancel_at
        }
        fn progress(&self, _: u64, _: u64) {}
    }

    #[test]
    fn streamed_files_are_bounded_validated_and_cancelled_without_replacing_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chapter.mp3");
        let mut audio = vec![0; CHUNK * 2];
        audio[..3].copy_from_slice(b"ID3");
        let live = || Watch { reads: Cell::new(0), cancel_at: usize::MAX };
        save_audio(&mut audio.as_slice(), &path, MAX_CHAPTER, audio.len() as u64, &live()).unwrap();
        assert!(cached_audio(&path, MAX_CHAPTER));
        for (limit, total, cancel) in [(1024, 0, usize::MAX), (MAX_CHAPTER, audio.len() as u64 + 1, usize::MAX), (MAX_CHAPTER, 0, 1)] {
            assert!(save_audio(&mut audio.as_slice(), &path, limit, total, &Watch { reads: Cell::new(0), cancel_at: cancel }).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), audio);
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        }
        assert!(save_audio(&mut b"<html>not audio</html>".as_slice(), &path, MAX_CHAPTER, 0, &live()).is_err());
    }

    #[test]
    fn timing_responses_stop_at_the_size_bound_or_cancel() {
        let bytes = vec![b' '; MAX_TIMING as usize + 1];
        let live = Watch { reads: Cell::new(0), cancel_at: usize::MAX };
        assert!(read_metadata(&mut bytes.as_slice(), &live).is_err());
        assert_eq!(read_metadata(&mut b"[]".as_slice(), &live).unwrap(), b"[]");
        assert!(read_metadata(&mut bytes.as_slice(), &Watch { reads: Cell::new(0), cancel_at: 1 }).is_err());
    }
}
