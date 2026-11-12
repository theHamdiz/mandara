//! Stash on disk. Every function here blocks: call them on a background
//! thread, never on the UI thread.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use super::model::{self, Eviction, StashState};

/// `$XDG_STATE_HOME/mandara/stash.json`.
pub fn state_path() -> PathBuf {
    mandara_config::paths::state_dir().join("stash.json")
}

/// `$XDG_CACHE_HOME/mandara/stash/`.
pub fn root() -> PathBuf {
    mandara_config::paths::cache_dir().join("stash")
}

/// One shelf's folder.
pub fn shelf_dir(shelf: u64) -> PathBuf {
    root().join(shelf.to_string())
}

/// A stash file's path.
pub fn blob_path(shelf: u64, blob: &str) -> PathBuf {
    shelf_dir(shelf).join(blob)
}

/// Loads the saved stash (an unreadable file is kept aside as
/// `stash.json.bad` and an empty stash starts).
pub fn load() -> StashState {
    load_from(&state_path())
}

pub fn load_from(path: &Path) -> StashState {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return StashState::default(),
        Err(e) => {
            tracing::warn!("stash: reading {}: {e}", path.display());
            return StashState::default();
        }
    };
    match serde_json::from_str::<StashState>(&text) {
        Ok(mut state) => {
            state.sanitize();
            state
        }
        Err(e) => {
            tracing::warn!("stash: {} is not a stash ({e}); kept as stash.json.bad", path.display());
            let _ = std::fs::rename(path, path.with_extension("json.bad"));
            StashState::default()
        }
    }
}

/// Writes the stash atomically (a crash never leaves half a file).
pub fn save(json: &str) -> anyhow::Result<()> {
    save_to(&state_path(), json)
}

pub fn save_to(path: &Path, json: &str) -> anyhow::Result<()> {
    mandara_config::write_atomic(path, json.as_bytes())
}

/// A new file in a shelf's folder `dir` with `bytes`, named `<stem>.<ext>`
/// (or `<stem>-2.<ext>`…, never overwriting). Returns its file name.
pub fn write_blob(dir: &Path, stem: &str, ext: &str, bytes: &[u8]) -> std::io::Result<String> {
    write_blob_in(dir, stem, ext, |f| f.write_all(bytes))
}

/// Like [`write_blob`], streaming from `reader` with at most `limit` bytes
/// (more fails and removes the file). Returns the name and size.
pub fn stream_blob(dir: &Path, stem: &str, ext: &str, reader: &mut dyn std::io::Read, limit: u64) -> std::io::Result<(String, u64)> {
    let mut written = 0;
    let name = write_blob_in(dir, stem, ext, |f| {
        written = std::io::copy(&mut reader.take(limit + 1), f)?;
        if written > limit {
            return Err(std::io::Error::other(format!("larger than {} MiB", limit / (1024 * 1024))));
        }
        Ok(())
    })?;
    Ok((name, written))
}

fn write_blob_in(dir: &Path, stem: &str, ext: &str, fill: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>) -> std::io::Result<String> {
    std::fs::create_dir_all(dir)?;
    let stem: String = stem.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).take(48).collect();
    let stem = if stem.is_empty() { "item".to_string() } else { stem };
    for n in 1..10_000u32 {
        let name = if n == 1 { format!("{stem}.{ext}") } else { format!("{stem}-{n}.{ext}") };
        let path = dir.join(&name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                if let Err(e) = fill(&mut f).and_then(|()| f.flush()) {
                    drop(f);
                    let _ = std::fs::remove_file(&path);
                    return Err(e);
                }
                return Ok(name);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no free file name"))
}

/// Reads up to `max` bytes of a stash file (previews, quick look).
pub fn read_head(path: &Path, max: usize) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(max.min(64 * 1024));
    std::fs::File::open(path)?.take(max as u64).read_to_end(&mut out)?;
    Ok(out)
}

/// Bytes in a shelf's folder (flat: no sub-folders).
pub fn dir_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir).map(|it| it.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum()).unwrap_or(0)
}

/// Removes a shelf's folder.
pub fn remove_shelf_dir(shelf: u64) {
    let dir = shelf_dir(shelf);
    if let Err(e) = std::fs::remove_dir_all(&dir)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("stash: removing {}: {e}", dir.display());
    }
}

/// Removes one stash file.
pub fn remove_blob(dir: &Path, blob: &str) {
    // A blob name never leaves its shelf's folder.
    if blob.contains('/') || blob.starts_with('.') {
        return;
    }
    let _ = std::fs::remove_file(dir.join(blob));
}

/// What the cache holds and decides under `limit`: removes the folders of
/// evicted closed shelves and of shelves the state no longer knows.
/// Returns the plan and the bytes the open shelves hold.
pub fn enforce_limit(open: &[u64], closed: &[(u64, u64)], limit: u64) -> (Eviction, u64) {
    let root = root();
    let open_bytes: u64 = open.iter().map(|id| dir_bytes(&shelf_dir(*id))).sum();
    let closed_sizes: Vec<(u64, u64, u64)> = closed.iter().map(|(id, at)| (*id, dir_bytes(&shelf_dir(*id)), *at)).collect();
    let plan = model::plan_eviction(open_bytes, &closed_sizes, limit);
    for id in &plan.evict {
        remove_shelf_dir(*id);
    }
    // Folders of shelves that are gone for good (an emptied close whose
    // removal was interrupted, a hand-edited state).
    if let Ok(entries) = std::fs::read_dir(&root) {
        for e in entries.flatten() {
            let Some(id) = e.file_name().to_str().and_then(|n| n.parse::<u64>().ok()) else { continue };
            if !open.contains(&id) && !closed.iter().any(|c| c.0 == id) {
                remove_shelf_dir(id);
            }
        }
    }
    (plan, open_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blobs_never_overwrite_and_streams_are_capped() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_blob_in(dir.path(), "pic", "png", |f| f.write_all(b"one")).unwrap();
        let b = write_blob_in(dir.path(), "pic", "png", |f| f.write_all(b"two")).unwrap();
        assert_eq!((a.as_str(), b.as_str()), ("pic.png", "pic-2.png"));
        assert_eq!(std::fs::read(dir.path().join("pic.png")).unwrap(), b"one");
        // Unsafe characters never reach the file system.
        let c = write_blob_in(dir.path(), "../../etc/passwd", "txt", |f| f.write_all(b"x")).unwrap();
        assert!(!c.contains('/'));
        assert!(dir.path().join(&c).is_file());
        // An oversized stream fails and leaves nothing behind.
        let before = std::fs::read_dir(dir.path()).unwrap().count();
        let err = write_blob_in(dir.path(), "big", "bin", |f| {
            let n = std::io::copy(&mut std::io::repeat(7).take(11), f)?;
            if n > 10 { Err(std::io::Error::other("too big")) } else { Ok(()) }
        });
        assert!(err.is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), before);
        assert_eq!(dir_bytes(dir.path()), 7);
    }

    #[test]
    fn state_files_round_trip_and_bad_ones_are_kept_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stash.json");
        assert_eq!(load_from(&path), StashState::default(), "no file: empty stash");
        let mut s = StashState::default();
        let id = s.take_id();
        s.shelves.push(model::Shelf::new(id, 5, Some("HDMI-A-1".into()), (12.0, 34.0), (320.0, 380.0), mandara_config::stash::ShelfView::List));
        save_to(&path, &serde_json::to_string(&s).unwrap()).unwrap();
        let mut back = load_from(&path);
        s.sanitize();
        assert_eq!(back, s);
        back.shelves.clear();
        std::fs::write(&path, b"{ not json").unwrap();
        assert_eq!(load_from(&path), StashState::default());
        assert!(path.with_extension("json.bad").is_file(), "the unreadable file is kept");
    }
}
