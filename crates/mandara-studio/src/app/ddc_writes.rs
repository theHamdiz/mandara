//! The brightness writers of Studio's DDC/CI sliders: one thread per monitor
//! that always sends the newest wanted value, replaced when a write hangs
//! (an I2C transfer stuck in the kernel) so the slider never goes dead.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A write older than this is treated as hung: the next slider move starts a
/// new writer. A healthy DDC/CI write takes well under a second.
pub(super) const WRITE_DEADLINE: Duration = Duration::from_secs(10);

/// One monitor's slider: the latest wanted percent, the writer allowed to send
/// it and when that writer's current write began.
struct Slot {
    percent: u32,
    generation: u64,
    started: Instant,
}

#[derive(Default)]
struct Slots {
    next_generation: u64,
    monitors: HashMap<String, Slot>,
}

/// What every monitor's writer is doing.
#[derive(Default)]
pub(super) struct DdcWrites(Mutex<Slots>);

impl DdcWrites {
    fn lock(&self) -> std::sync::MutexGuard<'_, Slots> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Records the wanted `percent`. `Some(generation)` when the caller must
    /// start a writer for it: the monitor has none, or its write in flight has
    /// run past `deadline` (the old writer then ends when it comes back).
    pub(super) fn want(&self, connector: &str, percent: u32, now: Instant, deadline: Duration) -> Option<u64> {
        let mut slots = self.lock();
        if let Some(slot) = slots.monitors.get_mut(connector)
            && now.saturating_duration_since(slot.started) < deadline
        {
            slot.percent = percent;
            return None;
        }
        slots.next_generation += 1;
        let generation = slots.next_generation;
        slots.monitors.insert(connector.to_string(), Slot { percent, generation, started: now });
        Some(generation)
    }

    /// The value writer `generation` sends next, with its write starting now;
    /// `None` when a newer writer replaced it.
    fn begin(&self, connector: &str, generation: u64, now: Instant) -> Option<u32> {
        let mut slots = self.lock();
        let slot = slots.monitors.get_mut(connector).filter(|s| s.generation == generation)?;
        slot.started = now;
        Some(slot.percent)
    }

    /// Writer `generation` sent `sent`. True when it must end: nothing newer
    /// was wanted (the monitor is idle again) or it was replaced meanwhile.
    fn finish(&self, connector: &str, generation: u64, sent: u32) -> bool {
        let mut slots = self.lock();
        match slots.monitors.get(connector) {
            Some(slot) if slot.generation == generation => {
                let idle = slot.percent == sent;
                if idle {
                    slots.monitors.remove(connector);
                }
                idle
            }
            _ => true,
        }
    }

    /// The writer loop for `generation`: `write` sends one percent.
    pub(super) fn run_writer(&self, connector: &str, generation: u64, mut write: impl FnMut(u32)) {
        while let Some(percent) = self.begin(connector, generation, Instant::now()) {
            write(percent);
            if self.finish(connector, generation, percent) {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::mpsc::channel;

    const DEADLINE: Duration = Duration::from_secs(10);

    #[test]
    fn one_writer_per_monitor_and_it_sends_the_newest_value() {
        let writes = DdcWrites::default();
        let now = Instant::now();
        let generation = writes.want("DP-1", 10, now, DEADLINE).expect("first move starts a writer");
        assert_eq!(writes.want("DP-1", 20, now, DEADLINE), None);
        assert_eq!(writes.want("DP-1", 30, now, DEADLINE), None);
        assert!(writes.want("HDMI-1", 5, now, DEADLINE).is_some(), "monitors do not share a writer");
        let mut sent = Vec::new();
        writes.run_writer("DP-1", generation, |p| sent.push(p));
        assert_eq!(sent, [30], "coalesced moves send only the newest value");
        assert!(writes.want("DP-1", 40, now, DEADLINE).is_some(), "an idle monitor starts a new writer");
    }

    #[test]
    fn a_value_wanted_during_a_write_is_sent_next() {
        let writes = Arc::new(DdcWrites::default());
        let now = Instant::now();
        let generation = writes.want("DP-1", 10, now, DEADLINE).unwrap();
        let mut sent = Vec::new();
        let w = writes.clone();
        writes.run_writer("DP-1", generation, |p| {
            sent.push(p);
            if p == 10 {
                assert_eq!(w.want("DP-1", 60, Instant::now(), DEADLINE), None);
            }
        });
        assert_eq!(sent, [10, 60]);
    }

    #[test]
    fn a_hung_writer_is_replaced_and_ends_when_it_returns() {
        let writes = Arc::new(DdcWrites::default());
        let start = Instant::now();
        let hung = writes.want("DP-1", 10, start, DEADLINE).unwrap();
        // The hung writer began its write at `start` and never returns yet.
        assert_eq!(writes.begin("DP-1", hung, start), Some(10));
        assert_eq!(writes.want("DP-1", 20, start + DEADLINE / 2, DEADLINE), None, "still within the deadline");
        let fresh = writes.want("DP-1", 30, start + DEADLINE, DEADLINE).expect("past the deadline a new writer starts");
        assert_ne!(fresh, hung);

        let (tx, rx) = channel();
        writes.run_writer("DP-1", fresh, |p| tx.send(p).unwrap());
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), [30]);

        // The hung write finally returns: it must neither resend nor remove
        // the newer writer's state.
        let again = writes.want("DP-1", 70, Instant::now(), DEADLINE).unwrap();
        assert!(writes.finish("DP-1", hung, 10), "a replaced writer ends");
        assert_eq!(writes.begin("DP-1", hung, Instant::now()), None);
        assert_eq!(writes.begin("DP-1", again, Instant::now()), Some(70), "the newer writer's want is untouched");
    }
}
