//! The last few recordings (path, length, size), kept in
//! `~/.local/state/mandara/recordings.json` for the screenshot panel's
//! "Recent recordings" list. Files deleted or moved since drop out when the
//! list is read; sizes are read fresh.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Recordings remembered.
pub const KEEP: usize = 8;

/// Held across every read-modify-write of the history, so a recording
/// saved while another is moved to the Trash can't drop either change
/// (and two writers never share `write_atomic`'s temporary file).
static WRITE: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recent {
    pub path: PathBuf,
    /// Length in seconds.
    pub secs: f64,
    /// Size in bytes (refreshed when listed).
    pub bytes: u64,
    /// When it was saved, seconds since the Unix epoch.
    pub at: i64,
}

impl Recent {
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.secs.max(0.0))
    }

    pub fn name(&self) -> String {
        self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    }
}

/// The history file.
pub fn file() -> PathBuf {
    mandara_config::paths::state_dir().join("recordings.json")
}

fn read(path: &Path) -> Vec<Recent> {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write(path: &Path, list: &[Recent]) -> Result<(), String> {
    let text = serde_json::to_vec_pretty(list).map_err(|e| e.to_string())?;
    mandara_config::write_atomic(path, &text).map_err(|e| e.to_string())
}

/// Recordings in `path` whose files still exist, newest first, with their
/// current size.
pub fn list(path: &Path) -> Vec<Recent> {
    read(path)
        .into_iter()
        .filter_map(|mut r| {
            let meta = std::fs::metadata(&r.path).ok().filter(|m| m.is_file())?;
            r.bytes = meta.len();
            Some(r)
        })
        .collect()
}

/// Adds a finished recording at the top (replacing an entry for the same
/// file) and keeps the newest [`KEEP`].
pub fn add(path: &Path, entry: Recent) -> Result<(), String> {
    let _held = WRITE.lock().map_err(|e| e.to_string())?;
    let mut list = list(path);
    list.retain(|r| r.path != entry.path);
    list.insert(0, entry);
    list.truncate(KEEP);
    write(path, &list)
}

/// Forgets `file` (it was deleted or moved to the Trash).
pub fn forget(path: &Path, file: &Path) -> Result<(), String> {
    let _held = WRITE.lock().map_err(|e| e.to_string())?;
    let mut list = read(path);
    let before = list.len();
    list.retain(|r| r.path != file);
    if list.len() == before { Ok(()) } else { write(path, &list) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_newest_existing_files() {
        let dir = std::env::temp_dir().join(format!("mandara-rec-history-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = dir.join("state").join("recordings.json");
        assert!(list(&store).is_empty(), "no file yet");
        let mut files = Vec::new();
        for i in 0..KEEP + 2 {
            let f = dir.join(format!("r{i}.mp4"));
            std::fs::write(&f, vec![0u8; i + 1]).unwrap();
            add(&store, Recent { path: f.clone(), secs: i as f64, bytes: 0, at: i as i64 }).unwrap();
            files.push(f);
        }
        let l = list(&store);
        assert_eq!(l.len(), KEEP);
        assert_eq!(l[0].path, files[KEEP + 1], "newest first");
        assert_eq!(l[0].bytes, (KEEP + 2) as u64, "size read fresh");
        // Re-adding a file moves it to the top instead of duplicating it.
        add(&store, Recent { path: files[KEEP].clone(), secs: 1.0, bytes: 0, at: 99 }).unwrap();
        let l = list(&store);
        assert_eq!(l.len(), KEEP);
        assert_eq!(l[0].at, 99);
        // A deleted file drops out; forgetting removes the entry.
        std::fs::remove_file(&files[KEEP + 1]).unwrap();
        assert!(list(&store).iter().all(|r| r.path != files[KEEP + 1]));
        forget(&store, &files[KEEP]).unwrap();
        assert!(list(&store).iter().all(|r| r.path != files[KEEP]));
        assert_eq!(l[0].name(), format!("r{KEEP}.mp4"));
        assert_eq!(l[0].duration(), Duration::from_secs(1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Saves and forgets running at once (a recording stopping while
    /// another goes to the Trash) never lose each other's change.
    #[test]
    fn concurrent_adds_and_forgets_keep_every_change() {
        let dir = std::env::temp_dir().join(format!("mandara-rec-history-race-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = dir.join("recordings.json");
        let files: Vec<PathBuf> = (0..KEEP).map(|i| dir.join(format!("c{i}.mkv"))).collect();
        for f in &files {
            std::fs::write(f, b"x").unwrap();
        }
        // The odd ones are listed first and forgotten while the even ones
        // are added.
        for f in files.iter().skip(1).step_by(2) {
            add(&store, Recent { path: f.clone(), secs: 1.0, bytes: 0, at: 0 }).unwrap();
        }
        std::thread::scope(|s| {
            for (i, f) in files.iter().enumerate() {
                let store = &store;
                s.spawn(move || {
                    if i % 2 == 0 {
                        add(store, Recent { path: f.clone(), secs: 1.0, bytes: 0, at: i as i64 }).unwrap();
                    } else {
                        forget(store, f).unwrap();
                    }
                });
            }
        });
        let mut got: Vec<PathBuf> = list(&store).into_iter().map(|r| r.path).collect();
        let mut want: Vec<PathBuf> = files.iter().step_by(2).cloned().collect();
        got.sort();
        want.sort();
        assert_eq!(got, want, "every add stays and every forget sticks");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
