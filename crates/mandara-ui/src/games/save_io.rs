//! Where the games keep their state and how it gets there: one small file
//! per game under `~/.local/state/mandara/games/`, read once off the UI
//! thread, written whole and atomically on the blocking pool, coalesced
//! (one write per pause in play, never per frame or per key) and with one
//! write in flight at a time. An unreadable or corrupted file is moved
//! aside as `<name>.corrupt` and never read again.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::{Context, Task};

/// Changes are written this long after the last one (a burst of moves is
/// one write).
pub const SAVE_DEBOUNCE: Duration = Duration::from_millis(900);
/// Suffix of a file that was set aside unread.
const CORRUPT_SUFFIX: &str = "corrupt";

/// `~/.local/state/mandara/games/<name>`.
pub fn path(name: &str) -> PathBuf {
    mandara_config::paths::state_dir().join("games").join(name)
}

/// What reading a save file found.
#[derive(Debug, PartialEq, Eq)]
pub enum Read {
    Missing,
    Bytes(Vec<u8>),
    Unreadable(String),
}

/// Reads `path` (blocking: call from the pool).
pub fn read(path: &Path) -> Read {
    match std::fs::read(path) {
        Ok(bytes) => Read::Bytes(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Read::Missing,
        Err(e) => Read::Unreadable(e.to_string()),
    }
}

/// Moves an untrusted file aside, replacing an older one (blocking).
pub fn quarantine(path: &Path) {
    let mut aside = path.as_os_str().to_owned();
    aside.push(".");
    aside.push(CORRUPT_SUFFIX);
    if let Err(e) = std::fs::rename(path, &aside) {
        tracing::warn!("games: setting {} aside: {e}", path.display());
    }
}

/// Writes `bytes` to `path` atomically (blocking).
pub fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    mandara_config::store::write_atomic(path, bytes).map_err(|e| format!("{}: {e:#}", path.display()))
}

/// A model that saves itself.
pub trait Persist: Sized + 'static {
    fn saver(&mut self) -> &mut Saver;
    /// The bytes to write now (tiny: serialised on the UI thread).
    fn snapshot(&self) -> Vec<u8>;
    fn file(&self) -> PathBuf;
}

/// Coalesced saving for one model.
#[derive(Default)]
pub struct Saver {
    dirty: bool,
    task: Option<Task<()>>,
    /// The last write failed (shown nowhere but the log; the next change retries).
    pub error: Option<String>,
}

impl Saver {
    pub fn dirty(&self) -> bool {
        self.dirty
    }
}

/// Marks `this` changed and makes sure one write follows.
pub fn changed<T: Persist>(this: &mut T, cx: &mut Context<T>) {
    let saver = this.saver();
    saver.dirty = true;
    if saver.task.is_some() {
        return;
    }
    let task = cx.spawn(async move |weak, cx| {
        loop {
            cx.background_executor().timer(SAVE_DEBOUNCE).await;
            let Ok(Some((file, bytes))) = weak.update(cx, |t, _| {
                let s = t.saver();
                if !s.dirty {
                    return None;
                }
                s.dirty = false;
                Some((t.file(), t.snapshot()))
            }) else {
                break;
            };
            let result = mandara_services::runtime::unblock(move || write(&file, &bytes)).await;
            let again = weak
                .update(cx, |t, _| {
                    let s = t.saver();
                    if let Err(e) = &result {
                        tracing::warn!("games: saving: {e}");
                    }
                    s.error = result.err();
                    if s.dirty {
                        true
                    } else {
                        s.task = None;
                        false
                    }
                })
                .unwrap_or(false);
            if !again {
                break;
            }
        }
    });
    this.saver().task = Some(task);
}

/// Writes the latest state now on the blocking pool, if anything changed
/// (the plugin was switched off and the model is going away).
pub fn flush_in_background<T: Persist>(this: &mut T, cx: &mut Context<T>) {
    if !this.saver().dirty {
        return;
    }
    let (file, bytes) = (this.file(), this.snapshot());
    let saver = this.saver();
    saver.dirty = false;
    saver.task = None;
    cx.spawn(async move |_, _| {
        if let Err(e) = mandara_services::runtime::unblock(move || write(&file, &bytes)).await {
            tracing::warn!("games: saving: {e}");
        }
    })
    .detach();
}

/// Writes now, if anything changed (the shell is restarting): blocking, a
/// few hundred bytes, like the config's own flush beside it.
pub fn flush<T: Persist>(this: &mut T) {
    if !this.saver().dirty {
        return;
    }
    let (file, bytes) = (this.file(), this.snapshot());
    let result = write(&file, &bytes);
    let saver = this.saver();
    saver.task = None;
    saver.dirty = result.is_err();
    if let Err(e) = &result {
        tracing::warn!("games: saving: {e}");
    }
    saver.error = result.err();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_round_trip_and_corrupt_ones_are_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("games").join("sudoku.json");
        assert_eq!(read(&file), Read::Missing);
        write(&file, b"{}").unwrap();
        assert_eq!(read(&file), Read::Bytes(b"{}".to_vec()));
        quarantine(&file);
        assert_eq!(read(&file), Read::Missing);
        assert_eq!(std::fs::read(dir.path().join("games").join("sudoku.json.corrupt")).unwrap(), b"{}");
    }
}
