//! Leave anonymous API capacity for explicit install and update actions.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Optional presentation uses at most a third of GitHub's anonymous hourly quota.
const REQUESTS: u32 = 20;
/// Stop before consuming the last requests an installation may need.
const RESERVED: u64 = 16;
/// GitHub's core REST rate-limit window.
const WINDOW: Duration = Duration::from_secs(60 * 60);
/// Optional metadata must not inherit the installer’s minute-long timeout.
pub(super) const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Allowance {
    api: String,
    used: u32,
    remaining: Option<u64>,
    reset: u64,
    blocked_until: u64,
}

impl Allowance {
    fn prepare(&mut self, api: &str, now: u64) {
        if self.api != api || (now >= self.reset && now >= self.blocked_until) {
            *self = Self { api: api.to_owned(), reset: now.saturating_add(WINDOW.as_secs()), ..Self::default() };
        }
    }
    fn available(&mut self, api: &str, now: u64) -> bool {
        self.prepare(api, now);
        now >= self.blocked_until && self.used < REQUESTS && self.remaining.is_none_or(|n| n > RESERVED)
    }
    fn take(&mut self, api: &str, now: u64) -> bool {
        if !self.available(api, now) { return false; }
        self.used += 1;
        self.remaining = self.remaining.map(|n| n.saturating_sub(1));
        true
    }
    fn record(&mut self, api: &str, now: u64, remaining: Option<u64>, reset: Option<u64>, wait: Option<u64>) {
        self.prepare(api, now);
        if let Some(remaining) = remaining { self.remaining = Some(self.remaining.map_or(remaining, |n| n.min(remaining))); }
        if let Some(reset) = reset.filter(|reset| *reset > now) { self.reset = reset; }
        if let Some(wait) = wait { self.blocked_until = now.saturating_add(wait); }
    }
}

static ALLOWANCE: parking_lot::Mutex<Option<Allowance>> = parking_lot::Mutex::new(None);
fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }

pub(super) fn available() -> bool {
    ALLOWANCE.lock().get_or_insert_with(Allowance::default).available(&crate::marketplace::api_base(), now())
}

pub(super) fn take() -> Result<(), String> {
    ALLOWANCE.lock().get_or_insert_with(Allowance::default).take(&crate::marketplace::api_base(), now())
        .then_some(()).ok_or_else(|| "plugin metadata request budget exhausted".to_owned())
}

/// All marketplace core responses inform the reserve, including explicit installs.
pub(in crate::marketplace) fn observe(url: &str, response: &ureq::http::Response<ureq::Body>) {
    let api = crate::marketplace::api_base();
    if !url.strip_prefix(&api).is_some_and(|path| path.starts_with("/repos/")) { return; }
    let remaining = crate::marketplace::header_u64(response, "x-ratelimit-remaining");
    let reset = crate::marketplace::header_u64(response, "x-ratelimit-reset");
    let wait = crate::marketplace::rate_limit_wait(response);
    ALLOWANCE.lock().get_or_insert_with(Allowance::default).record(&api, now(), remaining, reset, wait);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_metadata_is_bounded_and_keeps_the_install_reserve() {
        let mut allowance = Allowance::default();
        for _ in 0..REQUESTS { assert!(allowance.take("api", 1)); }
        assert!(!allowance.take("api", 1));
        assert!(allowance.take("api", WINDOW.as_secs() + 1));
        allowance.record("api", WINDOW.as_secs() + 1, Some(RESERVED + 1), None, None);
        assert!(allowance.take("api", WINDOW.as_secs() + 1));
        assert!(!allowance.take("api", WINDOW.as_secs() + 1));
    }

    #[test]
    fn rate_limits_stop_enrichment_until_the_server_reset() {
        let mut allowance = Allowance::default();
        allowance.record("api", 1, Some(0), Some(20), Some(19));
        assert!(!allowance.take("api", 2));
        assert!(!allowance.take("api", 19));
        assert!(allowance.take("api", 20));
    }
}
