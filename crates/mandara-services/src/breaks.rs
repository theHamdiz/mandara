//! Eye-break scheduling (the 20-20-20 rule): a short break after every
//! `every` of active work, a long one after every `long_every`, where only
//! time at the keyboard counts. Being away for at least a break's length
//! counts as that break; snoozing postpones the reminder, pausing stops the
//! clock for a while.
//!
//! [`Schedule`] is a pure state machine over monotonic instants: the shell
//! feeds it idle / active transitions and actions and asks it for the next
//! moment anything can happen ([`Schedule::wake_at`]), so nothing ticks in
//! between. [`Tally`] keeps today's taken / skipped counts in
//! `~/.local/state/mandara/breaks.json`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Short,
    Long,
}

/// The configured rhythm.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rules {
    /// Active time between short breaks.
    pub every: Duration,
    pub short: Duration,
    /// Active time between long breaks (`None`: no long breaks).
    pub long_every: Option<Duration>,
    pub long: Duration,
}

impl Rules {
    pub fn length(&self, kind: Kind) -> Duration {
        match kind {
            Kind::Short => self.short,
            Kind::Long => self.long,
        }
    }
}

/// Something that happened on its own (see [`Schedule::poll`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A break is due now: remind.
    Due(Kind),
    /// A break ran its full length (taken).
    Over(Kind),
    /// A reminder went unanswered while you kept working (skipped).
    Missed(Kind),
    /// The pause ended.
    Unpaused,
}

/// How a break ended, for today's counts.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Taken(Kind),
    Skipped(Kind),
}

#[derive(Clone, Debug)]
pub struct Schedule {
    /// Active time banked since the last short / long break.
    since_short: Duration,
    since_long: Duration,
    /// Counting active time since this instant (`None`: not counting).
    running_from: Option<Instant>,
    /// Away (idle) since.
    idle_from: Option<Instant>,
    pub paused_until: Option<Instant>,
    pub snoozed_until: Option<Instant>,
    /// A reminder is out, since when.
    pub pending: Option<(Kind, Instant)>,
    /// On a break: kind, start, end.
    pub on_break: Option<(Kind, Instant, Instant)>,
}

impl Schedule {
    pub fn new(now: Instant) -> Self {
        Self { since_short: Duration::ZERO, since_long: Duration::ZERO, running_from: Some(now), idle_from: None, paused_until: None, snoozed_until: None, pending: None, on_break: None }
    }

    /// Active time counts right now.
    pub fn counting(&self) -> bool {
        self.idle_from.is_none() && self.paused_until.is_none() && self.on_break.is_none()
    }

    pub fn idle(&self) -> bool {
        self.idle_from.is_some()
    }

    fn bank(&mut self, now: Instant) {
        if let Some(from) = self.running_from {
            let d = now.saturating_duration_since(from);
            self.since_short += d;
            self.since_long += d;
            self.running_from = Some(now);
        }
    }

    /// Banks the time so far and (re)starts or stops counting to match
    /// the state.
    fn sync(&mut self, now: Instant) {
        self.bank(now);
        self.running_from = self.counting().then_some(now);
    }

    fn reset(&mut self, kind: Kind) {
        self.since_short = Duration::ZERO;
        if kind == Kind::Long {
            self.since_long = Duration::ZERO;
        }
    }

    /// Active time since the last short and long break.
    pub fn active(&self, now: Instant) -> (Duration, Duration) {
        let running = self.running_from.map(|f| now.saturating_duration_since(f)).unwrap_or_default();
        (self.since_short + running, self.since_long + running)
    }

    /// The break that comes next: a long one when it is due no later than
    /// the next short one.
    pub fn next_kind(&self, rules: &Rules, now: Instant) -> Kind {
        let (s, l) = self.active(now);
        match rules.long_every {
            Some(le) if le.saturating_sub(l) <= rules.every.saturating_sub(s) => Kind::Long,
            _ => Kind::Short,
        }
    }

    /// Active time left before the next break.
    pub fn until_due(&self, rules: &Rules, now: Instant) -> Duration {
        let (s, l) = self.active(now);
        match (self.next_kind(rules, now), rules.long_every) {
            (Kind::Long, Some(le)) => le.saturating_sub(l),
            _ => rules.every.saturating_sub(s),
        }
    }

    /// Active time left before the next long break (`None`: no long breaks).
    pub fn until_long(&self, rules: &Rules, now: Instant) -> Option<Duration> {
        rules.long_every.map(|le| le.saturating_sub(self.active(now).1))
    }

    /// When the next reminder fires if you keep working (`None` while away,
    /// paused, on a break or with a reminder out).
    pub fn due_at(&self, rules: &Rules, now: Instant) -> Option<Instant> {
        if !self.counting() || self.pending.is_some() {
            return None;
        }
        let at = now + self.until_due(rules, now);
        Some(self.snoozed_until.map_or(at, |s| s.max(at)))
    }

    /// The next moment [`Self::poll`] can report something; `miss_after`
    /// is how long a reminder may go unanswered (`None`: forever).
    pub fn wake_at(&self, rules: &Rules, now: Instant, miss_after: Option<Duration>) -> Option<Instant> {
        [
            self.paused_until,
            self.on_break.map(|(_, _, end)| end),
            self.pending.zip(miss_after).filter(|_| self.counting()).map(|((_, since), m)| since + m),
            self.due_at(rules, now),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Advances to `now`: at most one event per call (call until `None`).
    pub fn poll(&mut self, rules: &Rules, now: Instant, miss_after: Option<Duration>) -> Option<Event> {
        if self.paused_until.is_some_and(|u| now >= u) {
            self.paused_until = None;
            self.sync(now);
            return Some(Event::Unpaused);
        }
        if let Some((kind, _, end)) = self.on_break
            && now >= end
        {
            self.on_break = None;
            self.reset(kind);
            // Counting resumes from the moment the break ended.
            self.running_from = None;
            self.sync(end.min(now));
            self.bank(now);
            return Some(Event::Over(kind));
        }
        if let (Some((kind, since)), Some(m)) = (self.pending, miss_after)
            && self.counting()
            && now.saturating_duration_since(since) >= m
        {
            self.pending = None;
            self.sync(now);
            self.reset(kind);
            return Some(Event::Missed(kind));
        }
        if self.snoozed_until.is_some_and(|u| now >= u) {
            self.snoozed_until = None;
        }
        if self.counting() && self.pending.is_none() && self.snoozed_until.is_none() && self.until_due(rules, now).is_zero() {
            let kind = self.next_kind(rules, now);
            self.pending = Some((kind, now));
            return Some(Event::Due(kind));
        }
        None
    }

    /// You went idle at `at` (the idle detector reports it a little late).
    pub fn went_idle(&mut self, at: Instant, now: Instant) {
        if self.idle_from.is_some() {
            return;
        }
        let at = at.min(now);
        if let Some(from) = self.running_from {
            self.bank(at.max(from));
        }
        self.running_from = None;
        self.idle_from = Some(at);
    }

    /// You are back: time away of at least a break's length counts as that
    /// break (answering a reminder that was out).
    pub fn came_back(&mut self, rules: &Rules, now: Instant) -> Option<Outcome> {
        let from = self.idle_from.take()?;
        let away = now.saturating_duration_since(from);
        let mut outcome = None;
        if self.on_break.is_none() {
            let kind = if rules.long_every.is_some() && away >= rules.long {
                Some(Kind::Long)
            } else if away >= rules.short {
                Some(Kind::Short)
            } else {
                None
            };
            if let Some(kind) = kind {
                self.reset(kind);
                self.snoozed_until = None;
                // A long break answers any reminder; a short one only a
                // short reminder.
                if let Some((pending, _)) = self.pending
                    && (kind == Kind::Long || pending == Kind::Short)
                {
                    self.pending = None;
                    outcome = Some(Outcome::Taken(pending));
                }
            }
        }
        self.sync(now);
        outcome
    }

    /// Starts a break now: `kind`, else the one that is due, else a short one.
    pub fn take(&mut self, rules: &Rules, kind: Option<Kind>, now: Instant) -> Kind {
        let kind = kind.or(self.pending.map(|(k, _)| k)).unwrap_or(Kind::Short);
        self.bank(now);
        self.on_break = Some((kind, now, now + rules.length(kind)));
        self.pending = None;
        self.snoozed_until = None;
        self.paused_until = None;
        self.sync(now);
        kind
    }

    /// Skips the break on screen, the reminder that is out, or else the
    /// next one: the clock starts over.
    pub fn skip(&mut self, rules: &Rules, now: Instant) -> Outcome {
        let kind = self.on_break.map(|(k, ..)| k).or(self.pending.map(|(k, _)| k)).unwrap_or_else(|| self.next_kind(rules, now));
        self.bank(now);
        self.reset(kind);
        self.on_break = None;
        self.pending = None;
        self.snoozed_until = None;
        self.sync(now);
        Outcome::Skipped(kind)
    }

    /// Ends the break or reminder and asks again in `d` at the earliest.
    pub fn snooze(&mut self, d: Duration, now: Instant) {
        self.bank(now);
        self.pending = None;
        self.on_break = None;
        self.snoozed_until = Some(now + d);
        self.sync(now);
    }

    /// Stops the clock and every reminder for `d`.
    pub fn pause(&mut self, d: Duration, now: Instant) {
        self.bank(now);
        self.paused_until = Some(now + d);
        self.pending = None;
        self.on_break = None;
        self.snoozed_until = None;
        self.sync(now);
    }

    pub fn resume(&mut self, now: Instant) {
        self.paused_until = None;
        self.sync(now);
    }
}

/// Today's counts, persisted.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tally {
    /// `YYYY-MM-DD` the counts belong to.
    pub date: String,
    pub taken: u32,
    pub skipped: u32,
}

impl Tally {
    /// Counts `outcome` on `today`, starting over on a new day.
    pub fn record(&mut self, today: &str, outcome: Outcome) {
        if self.date != today {
            *self = Tally { date: today.to_string(), ..Tally::default() };
        }
        match outcome {
            Outcome::Taken(_) => self.taken += 1,
            Outcome::Skipped(_) => self.skipped += 1,
        }
    }

    /// `(taken, skipped)` for `today`.
    pub fn on(&self, today: &str) -> (u32, u32) {
        if self.date == today { (self.taken, self.skipped) } else { (0, 0) }
    }
}

pub fn tally_path() -> PathBuf {
    dirs::state_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/state")).join("mandara").join("breaks.json")
}

pub fn load_tally() -> Tally {
    std::fs::read_to_string(tally_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save_tally(tally: &Tally) {
    let path = tally_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec(tally)
        && let Err(e) = mandara_config::write_atomic(&path, &json)
    {
        tracing::warn!("breaks: saving {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    fn rules() -> Rules {
        Rules { every: 20 * MIN, short: Duration::from_secs(20), long_every: Some(60 * MIN), long: 5 * MIN }
    }

    /// Runs `poll` until quiet, collecting events.
    fn drain(s: &mut Schedule, r: &Rules, now: Instant, miss: Option<Duration>) -> Vec<Event> {
        std::iter::from_fn(|| s.poll(r, now, miss)).collect()
    }

    #[test]
    fn active_time_brings_a_short_break() {
        let (r, t0) = (rules(), Instant::now());
        let mut s = Schedule::new(t0);
        assert_eq!(s.until_due(&r, t0 + 5 * MIN), 15 * MIN);
        assert_eq!(s.wake_at(&r, t0, None), Some(t0 + 20 * MIN), "the timer sleeps until the break is due");
        assert!(drain(&mut s, &r, t0 + 19 * MIN, None).is_empty());
        assert_eq!(drain(&mut s, &r, t0 + 20 * MIN, None), [Event::Due(Kind::Short)]);
        assert_eq!(s.due_at(&r, t0 + 20 * MIN), None, "one reminder at a time");
        let t = t0 + 20 * MIN;
        assert_eq!(s.take(&r, None, t), Kind::Short);
        assert!(!s.counting());
        assert_eq!(drain(&mut s, &r, t + Duration::from_secs(20), None), [Event::Over(Kind::Short)]);
        assert_eq!(s.until_due(&r, t + Duration::from_secs(20)), 20 * MIN, "the clock starts over after the break");
    }

    #[test]
    fn idle_time_does_not_count_and_long_absence_is_a_break() {
        let (r, t0) = (rules(), Instant::now());
        let mut s = Schedule::new(t0);
        s.went_idle(t0 + 10 * MIN, t0 + 10 * MIN + Duration::from_secs(5));
        assert_eq!(s.due_at(&r, t0 + 12 * MIN), None, "nothing is due while away");
        // Back after 10 s: not a break; the 10 minutes before still count.
        assert_eq!(s.came_back(&r, t0 + 10 * MIN + Duration::from_secs(10)), None);
        assert_eq!(s.until_due(&r, t0 + 10 * MIN + Duration::from_secs(10)), 10 * MIN);
        // Away for 30 s: counts as a short break, while the long clock goes on.
        let t1 = t0 + 15 * MIN;
        s.went_idle(t1, t1);
        assert_eq!(s.came_back(&r, t1 + Duration::from_secs(30)), None, "no reminder was out");
        let t2 = t1 + Duration::from_secs(30);
        assert_eq!(s.until_due(&r, t2), 20 * MIN);
        // 10 min, then 4 min 50 s of work before stepping away.
        assert_eq!(s.until_long(&r, t2), Some(45 * MIN + Duration::from_secs(10)));
        // A reminder that is out is answered by stepping away.
        assert_eq!(drain(&mut s, &r, t2 + 20 * MIN, None), [Event::Due(Kind::Short)]);
        s.went_idle(t2 + 20 * MIN, t2 + 20 * MIN);
        assert_eq!(s.came_back(&r, t2 + 21 * MIN), Some(Outcome::Taken(Kind::Short)));
        assert!(s.pending.is_none());
    }

    #[test]
    fn long_breaks_come_every_hour_and_reset_both_clocks() {
        let (r, t0) = (rules(), Instant::now());
        let mut s = Schedule::new(t0);
        let mut t = t0;
        for expected in [Kind::Short, Kind::Short, Kind::Long] {
            t += 20 * MIN;
            assert_eq!(drain(&mut s, &r, t, None), [Event::Due(expected)]);
            let kind = s.take(&r, None, t);
            t += r.length(kind);
            assert_eq!(drain(&mut s, &r, t, None), [Event::Over(expected)]);
        }
        assert_eq!(s.until_long(&r, t), Some(60 * MIN));
        assert_eq!(s.next_kind(&r, t), Kind::Short);
        // Five minutes away counts as the long break.
        let mut s = Schedule::new(t0);
        s.went_idle(t0 + 30 * MIN, t0 + 30 * MIN);
        s.came_back(&r, t0 + 36 * MIN);
        assert_eq!(s.until_long(&r, t0 + 36 * MIN), Some(60 * MIN));
        // No long breaks when switched off.
        let off = Rules { long_every: None, ..rules() };
        assert_eq!(Schedule::new(t0).next_kind(&off, t0 + 59 * MIN), Kind::Short);
    }

    #[test]
    fn snooze_postpones_and_skip_starts_over() {
        let (r, t0) = (rules(), Instant::now());
        let mut s = Schedule::new(t0);
        let due = t0 + 20 * MIN;
        assert_eq!(drain(&mut s, &r, due, None), [Event::Due(Kind::Short)]);
        s.snooze(5 * MIN, due);
        assert_eq!(s.wake_at(&r, due, None), Some(due + 5 * MIN));
        assert!(drain(&mut s, &r, due + 4 * MIN, None).is_empty());
        assert_eq!(drain(&mut s, &r, due + 5 * MIN, None), [Event::Due(Kind::Short)]);
        assert_eq!(s.skip(&r, due + 5 * MIN), Outcome::Skipped(Kind::Short));
        assert_eq!(s.until_due(&r, due + 5 * MIN), 20 * MIN);
        // Snoozing before anything is due only makes it later, never sooner.
        let mut s = Schedule::new(t0);
        s.snooze(5 * MIN, t0);
        assert_eq!(s.due_at(&r, t0), Some(t0 + 20 * MIN));
    }

    #[test]
    fn pause_stops_the_clock_and_reminders() {
        let (r, t0) = (rules(), Instant::now());
        let mut s = Schedule::new(t0);
        s.pause(60 * MIN, t0 + 10 * MIN);
        assert_eq!(s.due_at(&r, t0 + 30 * MIN), None);
        assert_eq!(s.wake_at(&r, t0 + 30 * MIN, None), Some(t0 + 70 * MIN));
        assert!(drain(&mut s, &r, t0 + 69 * MIN, None).is_empty());
        assert_eq!(drain(&mut s, &r, t0 + 70 * MIN, None), [Event::Unpaused]);
        assert_eq!(s.until_due(&r, t0 + 70 * MIN), 10 * MIN, "the paused hour did not count");
        let mut s = Schedule::new(t0);
        s.pause(60 * MIN, t0);
        s.resume(t0 + 5 * MIN);
        assert_eq!(s.until_due(&r, t0 + 5 * MIN), 20 * MIN);
    }

    #[test]
    fn unanswered_reminders_count_as_skipped() {
        let (r, t0) = (rules(), Instant::now());
        let mut s = Schedule::new(t0);
        let miss = Some(5 * MIN);
        assert_eq!(drain(&mut s, &r, t0 + 20 * MIN, miss), [Event::Due(Kind::Short)]);
        assert_eq!(s.wake_at(&r, t0 + 20 * MIN, miss), Some(t0 + 25 * MIN));
        assert_eq!(drain(&mut s, &r, t0 + 25 * MIN, miss), [Event::Missed(Kind::Short)]);
        assert_eq!(s.until_due(&r, t0 + 25 * MIN), 20 * MIN);
    }

    #[test]
    fn tally_starts_over_each_day() {
        let mut t = Tally::default();
        t.record("2026-11-02", Outcome::Taken(Kind::Short));
        t.record("2026-11-02", Outcome::Skipped(Kind::Long));
        assert_eq!(t.on("2026-11-02"), (1, 1));
        assert_eq!(t.on("2026-11-03"), (0, 0));
        t.record("2026-11-03", Outcome::Taken(Kind::Short));
        assert_eq!((t.taken, t.skipped, t.date.as_str()), (1, 0, "2026-11-03"));
    }
}
