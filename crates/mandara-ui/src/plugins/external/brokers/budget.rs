//! A per-plugin ceiling on reply delivery: a plugin that answers every reply
//! with a new request would otherwise wake the UI thread once per delivery
//! tick for as long as it likes. Each delivery that follows the last one
//! closely doubles the wait before the next, up to [`MAX_HOLD`]; a plugin
//! that pauses starts over.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// The least time between two deliveries to one plugin (one delivery tick).
const BASE: Duration = Duration::from_millis(8);
/// The most a runaway plugin waits between deliveries.
const MAX_HOLD: Duration = Duration::from_millis(512);

struct State {
    /// When the plugin may next be delivered to.
    ready_at: Instant,
    /// The wait that was set at the last delivery.
    hold: Duration,
    /// When that delivery happened.
    last: Instant,
}

#[derive(Default)]
pub(super) struct Budget(HashMap<String, State>);

impl Budget {
    /// When `plugin` may be delivered to, or `None` when it may now.
    pub fn wait_until(&self, plugin: &str, now: Instant) -> Option<Instant> {
        self.0.get(plugin).map(|s| s.ready_at).filter(|at| *at > now)
    }

    /// Records a delivery to `plugin` at `now` and sets the wait before the
    /// next.
    pub fn delivered(&mut self, plugin: &str, now: Instant) {
        let hold = match self.0.get(plugin) {
            // Back to back (within twice the previous wait, plus a tick of
            // slack): a loop. Wait twice as long.
            Some(s) if now.saturating_duration_since(s.last) <= s.hold * 2 + BASE => (s.hold * 2).min(MAX_HOLD),
            _ => BASE,
        };
        self.0.insert(plugin.to_string(), State { ready_at: now + hold, hold, last: now });
        // Plugins quiet for long are forgotten, so the table stays small.
        self.0.retain(|_, s| now.saturating_duration_since(s.last) <= MAX_HOLD * 4);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_loop_backs_off_and_a_pause_resets_it() {
        let t0 = Instant::now();
        let mut b = Budget::default();
        assert_eq!(b.wait_until("p", t0), None, "never delivered: no wait");
        let mut now = t0;
        let mut holds = Vec::new();
        for _ in 0..10 {
            b.delivered("p", now);
            let at = b.wait_until("p", now).expect("a wait is set");
            holds.push(at - now);
            now = at;
        }
        assert_eq!(holds[0], BASE, "the first delivery waits one tick");
        assert_eq!(holds[1], BASE * 2, "a second one right behind it waits longer");
        assert!(holds.windows(2).all(|w| w[1] >= w[0]));
        assert_eq!(*holds.last().unwrap(), MAX_HOLD, "capped");
        // Quiet for a while, then one more: fresh.
        now += MAX_HOLD * 3;
        b.delivered("p", now);
        assert_eq!(b.wait_until("p", now), Some(now + BASE));
        // Another plugin is unaffected.
        assert_eq!(b.wait_until("q", now), None);
    }
}
