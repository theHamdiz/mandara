//! Offline mode (`general.offline`): one switch every network request of
//! Mandara's own checks first — weather, lyrics, album art, Wallhaven, the
//! plugin marketplace, public IP / speed test, update news, model and font
//! downloads.
//!
//! The process that owns the config (shell, Studio) calls [`set`] whenever
//! it changes. Anything else (a CLI one-shot) reads `config.toml` once on
//! first use.

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::SystemTime;

use parking_lot::Mutex;

/// Not told by the config owner: follow `config.toml` on disk.
const FOLLOW_FILE: u8 = 0;
const ONLINE: u8 = 1;
const OFFLINE: u8 = 2;

static STATE: AtomicU8 = AtomicU8::new(FOLLOW_FILE);
/// `config.toml`'s modification time and its `general.offline` when last read.
static FILE: Mutex<Option<(Option<SystemTime>, bool)>> = Mutex::new(None);

/// Follows `general.offline` (the process that owns the config calls this).
pub fn set(offline: bool) {
    let next = if offline { OFFLINE } else { ONLINE };
    if STATE.swap(next, Ordering::Relaxed) != next {
        tracing::info!("offline mode {}", if offline { "on" } else { "off" });
    }
}

/// `general.offline` from `config.toml`, re-read only when the file changed
/// (a `stat` per request otherwise).
fn from_file() -> bool {
    let path = mandara_config::paths::config_file();
    let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    let mut cache = FILE.lock();
    if let Some((seen, value)) = *cache
        && seen == mtime
    {
        return value;
    }
    let value = std::fs::read_to_string(&path).ok().and_then(|text| toml::from_str::<mandara_config::ShellConfig>(&text).ok()).is_some_and(|cfg| cfg.general.offline);
    *cache = Some((mtime, value));
    value
}

/// Whether offline mode is on.
pub fn is_offline() -> bool {
    match STATE.load(Ordering::Relaxed) {
        FOLLOW_FILE => from_file(),
        state => state == OFFLINE,
    }
}

/// The error every blocked request reports.
pub fn blocked() -> String {
    mandara_i18n::t("Offline mode is on (Studio → Privacy)").into()
}

/// `Err` while offline: call before any network request.
pub fn check() -> Result<(), String> {
    if is_offline() { Err(blocked()) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_blocks_requests() {
        set(true);
        assert!(is_offline());
        assert!(check().is_err());
        // Every lookup Studio and the shell offer stays off the network.
        assert_eq!(crate::location::search("Alexandria", "en").unwrap_err(), blocked());
        assert_eq!(crate::location::locate_by_ip().unwrap_err(), blocked());
        assert_eq!(crate::marketplace::search("mandara").map(|_| ()).unwrap_err(), blocked());
        set(false);
        assert!(check().is_ok());
    }
}
