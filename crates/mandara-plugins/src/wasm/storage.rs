//! A plugin's private key-value storage (the `storage` import): read once
//! on first use, kept in the plugin's store and written back by the host
//! off the UI thread after a call that changed it. Bounded in keys, key
//! and value length and total bytes; another plugin can't reach it.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::extension::{MAX_STORAGE_BYTES, MAX_STORAGE_KEY, MAX_STORAGE_KEYS, MAX_STORAGE_VALUE};

/// The most bytes JSON uses for one stored byte (`\u0000`), and its
/// overhead per key (quotes, colon, comma).
const JSON_ESCAPE: usize = 6;
const JSON_PER_KEY: usize = 8;

/// Snapshots handed to the host that haven't reached the disk yet (being
/// written, or kept after a failed write), newest per file. A plugin that
/// reloads before its write lands starts from here, not from the older file,
/// so a later edit can't build on stale data.
static UNWRITTEN: Mutex<Option<HashMap<PathBuf, Arc<str>>>> = Mutex::new(None);

fn unwritten<R>(f: impl FnOnce(&mut HashMap<PathBuf, Arc<str>>) -> R) -> R {
    f(UNWRITTEN.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new))
}

pub struct Storage {
    path: PathBuf,
    map: Option<BTreeMap<String, String>>,
    bytes: usize,
    dirty: bool,
}

impl Storage {
    /// Plugin `id`'s storage file (`id` is a validated plugin id, so it is
    /// a plain file name).
    pub fn new(id: &str) -> Self {
        Self { path: mandara_config::paths::data_dir().join("plugin-storage").join(format!("{id}.json")), map: None, bytes: 0, dirty: false }
    }

    fn map(&mut self) -> &mut BTreeMap<String, String> {
        if self.map.is_none() {
            // A small file read once per instance; anything unreadable or
            // over the limits starts empty.
            let pending = unwritten(|u| u.get(&self.path).cloned());
            let map: BTreeMap<String, String> = pending
                .map(|json| json.as_bytes().to_vec())
                .or_else(|| std::fs::read(&self.path).ok())
                // JSON may escape a byte as six (`\u0000`), plus quotes,
                // separators and braces per key.
                .filter(|b| b.len() <= MAX_STORAGE_BYTES * JSON_ESCAPE + MAX_STORAGE_KEYS * JSON_PER_KEY)
                .and_then(|b| serde_json::from_slice(&b).ok())
                .filter(within_limits)
                .unwrap_or_default();
            self.bytes = map.iter().map(|(k, v)| k.len() + v.len()).sum();
            self.map = Some(map);
        }
        self.map.as_mut().expect("loaded above")
    }

    pub fn get(&mut self, key: &str) -> Option<String> {
        self.map().get(key).cloned()
    }

    pub fn set(&mut self, key: String, value: String) -> Result<(), String> {
        if key.is_empty() || key.len() > MAX_STORAGE_KEY || key.chars().any(char::is_control) {
            return Err(format!("keys are 1–{MAX_STORAGE_KEY} bytes without control characters"));
        }
        if value.len() > MAX_STORAGE_VALUE {
            return Err(format!("a value may be at most {MAX_STORAGE_VALUE} bytes"));
        }
        let old = self.map().get(&key).map(|v| key.len() + v.len()).unwrap_or(0);
        let bytes = self.bytes - old + key.len() + value.len();
        if bytes > MAX_STORAGE_BYTES {
            return Err(format!("storage is full ({MAX_STORAGE_BYTES} bytes)"));
        }
        let new_key = old == 0 && !self.map().contains_key(&key);
        if new_key && self.map().len() >= MAX_STORAGE_KEYS {
            return Err(format!("storage holds at most {MAX_STORAGE_KEYS} keys"));
        }
        self.map().insert(key, value);
        self.bytes = bytes;
        self.dirty = true;
        Ok(())
    }

    pub fn remove(&mut self, key: &str) {
        if let Some(v) = self.map().remove(key) {
            self.bytes -= key.len() + v.len();
            self.dirty = true;
        }
    }

    pub fn keys(&mut self) -> Vec<String> {
        self.map().keys().cloned().collect()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// The file and its new contents, once, after a change. The snapshot
    /// counts as this file's contents until [`write_snapshot`] lands it.
    pub fn take_flush(&mut self) -> Option<(PathBuf, Arc<str>)> {
        if !std::mem::take(&mut self.dirty) {
            return None;
        }
        let json: Arc<str> = serde_json::to_string(self.map.as_ref()?).ok()?.into();
        unwritten(|u| u.insert(self.path.clone(), json.clone()));
        Some((self.path.clone(), json))
    }
}

/// Whether a loaded file keeps every limit `set` enforces (a file edited
/// by hand, or written by another version, could break them).
fn within_limits(map: &BTreeMap<String, String>) -> bool {
    map.len() <= MAX_STORAGE_KEYS
        && map.iter().all(|(k, v)| !k.is_empty() && k.len() <= MAX_STORAGE_KEY && !k.chars().any(char::is_control) && v.len() <= MAX_STORAGE_VALUE)
        && map.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>() <= MAX_STORAGE_BYTES
}

/// Writes a snapshot from [`Storage::take_flush`]; once it is on disk it no
/// longer stands in for the file (unless a newer snapshot came since).
pub fn write_snapshot(path: &Path, json: &Arc<str>) -> std::io::Result<()> {
    write(path, json)?;
    unwritten(|u| {
        if u.get(path).is_some_and(|newest| Arc::ptr_eq(newest, json)) {
            u.remove(path);
        }
    });
    Ok(())
}

/// Writes a flushed storage file atomically (a temporary file renamed over
/// it). Run off the UI thread.
pub fn write(path: &Path, json: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Unique per process and write: the shell and Studio may both hold the
    // plugin; the last write wins whole, never a mix.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let tmp = path.with_extension(format!("{}.{}.tmp", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    std::fs::write(&tmp, json)?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaped_values_survive_a_reload() {
        let dir = std::env::temp_dir().join(format!("mandara-storage-{}", std::process::id()));
        let path = dir.join("p.json");
        let mut s = Storage { path: path.clone(), map: Some(BTreeMap::new()), bytes: 0, dirty: false };
        for i in 0..6 {
            s.set(format!("k{i}"), "\0".repeat(MAX_STORAGE_VALUE - 8)).unwrap();
        }
        let (p, json) = s.take_flush().unwrap();
        assert!(json.len() > MAX_STORAGE_BYTES * 2, "the file is larger than the decoded limit");
        write_snapshot(&p, &json).unwrap();
        assert!(!unwritten(|u| u.contains_key(&p)), "the reload below reads the file itself");
        let mut again = Storage { path, map: None, bytes: 0, dirty: false };
        assert_eq!(again.keys().len(), 6, "nothing is lost on reload");
        for i in 0..6 {
            assert_eq!(again.get(&format!("k{i}")), Some("\0".repeat(MAX_STORAGE_VALUE - 8)), "values come back unchanged");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A file within the encoded size but over a decoded limit starts empty.
    #[test]
    fn files_over_the_decoded_limits_start_empty() {
        let dir = std::env::temp_dir().join(format!("mandara-storage-limits-{}", std::process::id()));
        let path = dir.join("p.json");
        let over_value = BTreeMap::from([("k".to_string(), "v".repeat(MAX_STORAGE_VALUE + 1))]);
        let over_total: BTreeMap<String, String> = (0..MAX_STORAGE_BYTES / MAX_STORAGE_VALUE + 1).map(|i| (format!("k{i}"), "v".repeat(MAX_STORAGE_VALUE - 8))).collect();
        let bad_key = BTreeMap::from([("a\u{7}".to_string(), "v".to_string())]);
        let empty_key = BTreeMap::from([(String::new(), "v".to_string())]);
        let long_key = BTreeMap::from([("k".repeat(MAX_STORAGE_KEY + 1), "v".to_string())]);
        let too_many: BTreeMap<String, String> = (0..=MAX_STORAGE_KEYS).map(|i| (format!("k{i}"), String::new())).collect();
        for map in [over_value, over_total, bad_key, empty_key, long_key, too_many] {
            write(&path, &serde_json::to_string(&map).unwrap()).unwrap();
            let mut s = Storage { path: path.clone(), map: None, bytes: 0, dirty: false };
            assert!(s.keys().is_empty(), "refused on load");
        }
        let good = BTreeMap::from([("k".to_string(), "v".to_string())]);
        write(&path, &serde_json::to_string(&good).unwrap()).unwrap();
        assert_eq!(Storage { path, map: None, bytes: 0, dirty: false }.get("k").as_deref(), Some("v"));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A plugin reloaded before its write lands (or after it failed) starts
    /// from the unwritten snapshot, so its next edit keeps the earlier one.
    #[test]
    fn a_reload_starts_from_the_unwritten_snapshot() {
        let dir = std::env::temp_dir().join(format!("mandara-storage-pending-{}", std::process::id()));
        let path = dir.join("p.json");
        write(&path, "{}").unwrap();
        let mut first = Storage { path: path.clone(), map: None, bytes: 0, dirty: false };
        first.set("a".into(), "1".into()).unwrap();
        let (_, failed) = first.take_flush().unwrap();
        // The write failed (never landed); the plugin unloads and reloads.
        drop(first);
        let mut second = Storage { path: path.clone(), map: None, bytes: 0, dirty: false };
        assert_eq!(second.get("a").as_deref(), Some("1"), "the unwritten edit is there");
        second.set("b".into(), "2".into()).unwrap();
        let (p, newest) = second.take_flush().unwrap();
        // A late landing of the older snapshot doesn't clear the newer one.
        write_snapshot(&p, &failed).unwrap();
        assert!(unwritten(|u| u.contains_key(&path)));
        // The disk recovers: the newest snapshot lands with both edits.
        write_snapshot(&p, &newest).unwrap();
        assert!(!unwritten(|u| u.contains_key(&path)));
        let mut third = Storage { path, map: None, bytes: 0, dirty: false };
        assert_eq!((third.get("a").as_deref(), third.get("b").as_deref()), (Some("1"), Some("2")));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A write that really fails (its folder can't be made: a file stands
    /// in the way) keeps the snapshot; a reload and a new edit build on it,
    /// and once the disk recovers both edits land.
    #[test]
    fn a_real_write_error_loses_nothing_across_a_reload() {
        let dir = std::env::temp_dir().join(format!("mandara-storage-error-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let blocker = dir.join("blocked");
        std::fs::write(&blocker, "not a folder").unwrap();
        let path = blocker.join("p.json");
        let mut first = Storage { path: path.clone(), map: None, bytes: 0, dirty: false };
        first.set("a".into(), "1".into()).unwrap();
        let (p, json) = first.take_flush().unwrap();
        assert!(write_snapshot(&p, &json).is_err(), "the write fails");
        drop(first);
        let mut second = Storage { path: path.clone(), map: None, bytes: 0, dirty: false };
        assert_eq!(second.get("a").as_deref(), Some("1"), "the reload keeps the failed edit");
        second.set("b".into(), "2".into()).unwrap();
        let (p, json) = second.take_flush().unwrap();
        assert!(write_snapshot(&p, &json).is_err(), "still failing");
        // The disk recovers; the kept snapshot is written again.
        std::fs::remove_file(&blocker).unwrap();
        write_snapshot(&p, &json).unwrap();
        assert!(!unwritten(|u| u.contains_key(&path)));
        let mut third = Storage { path, map: None, bytes: 0, dirty: false };
        assert_eq!((third.get("a").as_deref(), third.get("b").as_deref()), (Some("1"), Some("2")), "both edits are on disk");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn storage_is_bounded_and_flushes_once() {
        let mut s = Storage { path: PathBuf::from("/nonexistent/x.json"), map: Some(BTreeMap::new()), bytes: 0, dirty: false };
        s.set("a".into(), "1".into()).unwrap();
        assert_eq!(s.get("a").as_deref(), Some("1"));
        assert!(s.set(String::new(), "x".into()).is_err());
        assert!(s.set("k".into(), "x".repeat(MAX_STORAGE_VALUE + 1)).is_err());
        for i in 0..MAX_STORAGE_BYTES / MAX_STORAGE_VALUE {
            s.set(format!("big{i}"), "x".repeat(MAX_STORAGE_VALUE - 8)).unwrap();
        }
        assert!(s.set("one-more".into(), "x".repeat(MAX_STORAGE_VALUE)).unwrap_err().contains("full"));
        assert!(s.take_flush().is_some() && s.take_flush().is_none());
        s.remove("a");
        assert_eq!(s.get("a"), None);
        assert!(s.take_flush().is_some());
    }
}
