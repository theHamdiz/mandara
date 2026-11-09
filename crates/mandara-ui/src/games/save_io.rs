//! Where the games keep their state and how it gets there: one small file
//! per game under `~/.local/state/mandara/games/`, read once off the UI
//! thread, written whole and atomically on the blocking pool, coalesced
//! (one write per pause in play, never per frame or per key) and with one
//! write in flight at a time. Writes to one file happen one after another
//! in the order their snapshots were taken, so a write still running when
//! a flush starts can never put an older game back. An unreadable or
//! corrupted file is moved aside as `<name>.corrupt` and never read again.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
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

/// A snapshot's place in line, taken on the UI thread together with the
/// snapshot: a later snapshot always has a larger one.
fn ticket() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Writes snapshot `ticket` of `path` (blocking), unless a later snapshot
/// of the same file is already on disk. Writes are serialised, so the
/// check and the write can't interleave with another write.
fn write_in_order(path: &Path, ticket: u64, bytes: &[u8]) -> Result<(), String> {
    /// The newest snapshot written to each file (a game or two).
    static WRITTEN: Mutex<Vec<(PathBuf, u64)>> = Mutex::new(Vec::new());
    let mut written = WRITTEN.lock().unwrap_or_else(PoisonError::into_inner);
    let i = match written.iter().position(|(p, _)| p == path) {
        Some(i) => i,
        None => {
            written.push((path.to_path_buf(), 0));
            written.len() - 1
        }
    };
    if written[i].1 >= ticket {
        return Ok(());
    }
    write(path, bytes)?;
    written[i].1 = ticket;
    Ok(())
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
            let Ok(Some(snap)) = weak.update(cx, |t, _| {
                let s = t.saver();
                if !s.dirty {
                    return None;
                }
                s.dirty = false;
                Some((t.file(), ticket(), t.snapshot()))
            }) else {
                break;
            };
            let (file, n, bytes) = snap;
            let result = mandara_services::runtime::unblock(move || write_in_order(&file, n, &bytes)).await;
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
    let (file, n, bytes) = (this.file(), ticket(), this.snapshot());
    let saver = this.saver();
    saver.dirty = false;
    saver.task = None;
    cx.spawn(async move |_, _| {
        if let Err(e) = mandara_services::runtime::unblock(move || write_in_order(&file, n, &bytes)).await {
            tracing::warn!("games: saving: {e}");
        }
    })
    .detach();
}

/// Writes now, if anything changed or a write is still under way (the shell
/// is restarting): blocking, a few hundred bytes, like the config's own flush
/// beside it. A write in flight may not finish before the process exits, so a
/// fresh snapshot is written too; its newer ticket keeps the order.
pub fn flush<T: Persist>(this: &mut T) {
    if !this.saver().dirty && this.saver().task.is_none() {
        return;
    }
    let (file, n, bytes) = (this.file(), ticket(), this.snapshot());
    let result = write_in_order(&file, n, &bytes);
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

    struct Model { saver: Saver, file: PathBuf, state: Vec<u8> }

    impl Persist for Model {
        fn saver(&mut self) -> &mut Saver { &mut self.saver }
        fn snapshot(&self) -> Vec<u8> { self.state.clone() }
        fn file(&self) -> PathBuf { self.file.clone() }
    }

    /// A restart while the debounced write is still running (its snapshot
    /// taken, so nothing reads as dirty) still writes the latest state.
    #[test]
    fn a_flush_during_a_write_in_flight_still_writes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("games").join("2048.json");
        let mut model = Model { saver: Saver::default(), file: file.clone(), state: b"latest".to_vec() };
        flush(&mut model);
        assert_eq!(read(&file), Read::Missing, "nothing changed, nothing in flight: no write");
        model.saver.task = Some(Task::ready(()));
        flush(&mut model);
        assert_eq!(read(&file), Read::Bytes(b"latest".to_vec()));
        assert!(model.saver.task.is_none() && !model.saver.dirty());
    }

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

    /// A debounced write that is still running when a flush writes a newer
    /// snapshot finishes last without putting the older game back.
    #[test]
    fn an_older_snapshot_never_overwrites_a_newer_one() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("games").join("2048.json");
        let (older, newer) = (ticket(), ticket());
        write_in_order(&file, newer, b"newer").unwrap();
        write_in_order(&file, older, b"older").unwrap();
        assert_eq!(read(&file), Read::Bytes(b"newer".to_vec()));
        // Another file keeps its own order.
        let other = dir.path().join("games").join("sudoku.json");
        write_in_order(&other, older, b"older").unwrap();
        assert_eq!(read(&other), Read::Bytes(b"older".to_vec()));
        let latest = ticket();
        write_in_order(&file, latest, b"latest").unwrap();
        assert_eq!(read(&file), Read::Bytes(b"latest".to_vec()));
    }
}
