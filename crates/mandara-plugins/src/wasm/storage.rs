//! A plugin's private key-value storage (the `storage` import): read once
//! on first use, kept in the plugin's store and written back by the host
//! off the UI thread after a call that changed it. Bounded in keys, key
//! and value length and total bytes; another plugin can't reach it.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::extension::{MAX_STORAGE_BYTES, MAX_STORAGE_KEY, MAX_STORAGE_KEYS, MAX_STORAGE_VALUE};

/// The most bytes JSON uses for one stored byte (`\u0000`), and its
/// overhead per key (quotes, colon, comma).
const JSON_ESCAPE: usize = 6;
const JSON_PER_KEY: usize = 8;

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
            let map: BTreeMap<String, String> = std::fs::read(&self.path)
                .ok()
                // JSON may escape a byte as six (`\u0000`), plus quotes,
                // separators and braces per key.
                .filter(|b| b.len() <= MAX_STORAGE_BYTES * JSON_ESCAPE + MAX_STORAGE_KEYS * JSON_PER_KEY)
                .and_then(|b| serde_json::from_slice(&b).ok())
                .filter(|m: &BTreeMap<String, String>| m.len() <= MAX_STORAGE_KEYS)
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

    /// The file and its new contents, once, after a change.
    pub fn take_flush(&mut self) -> Option<(PathBuf, String)> {
        if !std::mem::take(&mut self.dirty) {
            return None;
        }
        let json = serde_json::to_string(self.map.as_ref()?).ok()?;
        Some((self.path.clone(), json))
    }
}

/// Writes a flushed storage file atomically (a temporary file renamed over
/// it). Run off the UI thread.
pub fn write(path: &std::path::Path, json: &str) -> std::io::Result<()> {
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
        write(&p, &json).unwrap();
        let mut again = Storage { path, map: None, bytes: 0, dirty: false };
        assert_eq!(again.keys().len(), 6, "nothing is lost on reload");
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
