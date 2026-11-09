//! Reddit's request budget. Every reply says how many requests remain in
//! the current window and when it resets; a 429 adds a growing pause.

use std::time::{Duration, Instant};

/// Keep a couple of requests in hand so an explicit refresh still works
/// when a burst of browsing nearly used the window up.
const RESERVE: f32 = 1.0;
/// The first pause after a 429 without a usable hint, doubled on each
/// further 429 up to the longest pause.
const FIRST_BACKOFF: Duration = Duration::from_secs(10);
const MAX_BACKOFF: Duration = Duration::from_secs(300);
/// A reset hint longer than this is not a rate-limit window.
const MAX_WINDOW: Duration = Duration::from_secs(900);

/// The rate-limit headers of one reply.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Limits {
    pub used: Option<f32>,
    pub remaining: Option<f32>,
    /// Seconds until the window resets.
    pub reset: Option<f32>,
    /// `Retry-After`, in seconds.
    pub retry_after: Option<f32>,
}

impl Limits {
    /// Read the headers by name; values that aren't finite numbers are ignored.
    pub fn from_headers<'a>(mut header: impl FnMut(&str) -> Option<&'a str>) -> Self {
        let mut number = |name: &str| header(name).and_then(|value| value.trim().parse::<f32>().ok()).filter(|value| value.is_finite() && *value >= 0.0);
        Self { used: number("x-ratelimit-used"), remaining: number("x-ratelimit-remaining"), reset: number("x-ratelimit-reset"), retry_after: number("retry-after") }
    }

    fn seconds(value: Option<f32>) -> Option<Duration> {
        // Clamped before the conversion, which panics past `Duration::MAX`.
        value.map(|secs| Duration::from_secs_f32(secs.min(MAX_WINDOW.as_secs_f32())))
    }
}

#[derive(Debug, Default)]
pub struct Budget {
    remaining: Option<f32>,
    used: Option<f32>,
    reset_at: Option<Instant>,
    paused_until: Option<Instant>,
    strikes: u32,
}

impl Budget {
    /// How long to wait before the next API request, if at all.
    pub fn wait(&self, now: Instant) -> Option<Duration> {
        if let Some(until) = self.paused_until.filter(|until| *until > now) {
            return Some(until - now);
        }
        let reset = self.reset_at.filter(|reset| *reset > now)?;
        self.remaining.filter(|remaining| *remaining < RESERVE).map(|_| reset - now)
    }

    /// Record a reply's headers and status.
    pub fn observe(&mut self, status: u16, limits: Limits, now: Instant) {
        if limits.remaining.is_some() { self.remaining = limits.remaining; }
        if limits.used.is_some() { self.used = limits.used; }
        if let Some(reset) = Limits::seconds(limits.reset) { self.reset_at = Some(now + reset); }
        if status == 429 {
            self.strikes = self.strikes.saturating_add(1);
            let growing = FIRST_BACKOFF.saturating_mul(1 << self.strikes.saturating_sub(1).min(8)).min(MAX_BACKOFF);
            let hinted = Limits::seconds(limits.retry_after).into_iter().chain(Limits::seconds(limits.reset)).max().unwrap_or_default();
            self.paused_until = Some(now + growing.max(hinted).min(MAX_BACKOFF));
            self.remaining = Some(0.0);
        } else if (200..300).contains(&status) {
            self.strikes = 0;
            self.paused_until = None;
        }
    }

    /// Requests left in this window, as Reddit last reported it.
    pub fn remaining(&self, now: Instant) -> Option<f32> {
        self.remaining.filter(|_| self.reset_at.is_none_or(|reset| reset > now))
    }

    pub fn used(&self) -> Option<f32> { self.used }

    /// Seconds until the window resets.
    pub fn resets_in(&self, now: Instant) -> Option<Duration> {
        self.reset_at.filter(|reset| *reset > now).map(|reset| reset - now)
    }
}
