//! Fault isolation: every call into plugin code (builders, views, commands,
//! click handlers) runs inside [`guard`], so a panicking plugin is logged
//! and disabled instead of taking the shell down.

use std::panic::{AssertUnwindSafe, catch_unwind};

/// Runs `f`, returning `None` (and logging) if it panics.
pub fn guard<T>(what: &str, f: impl FnOnce() -> T) -> Option<T> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => Some(v),
        Err(payload) => {
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".into());
            tracing::error!(target: "mandara::guard", "{what} crashed and was isolated: {msg}");
            None
        }
    }
}

/// Routes panic messages (with location) to the log instead of only stderr.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let location = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
        tracing::error!(target: "mandara::panic", "panic in thread '{thread}' at {location}: {msg}");
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolates_panics() {
        assert_eq!(guard("ok", || 7), Some(7));
        assert_eq!(guard("boom", || -> i32 { panic!("plugin bug") }), None);
        let v: Vec<i32> = Vec::new();
        assert_eq!(guard("index", || v[3]), None);
    }
}
