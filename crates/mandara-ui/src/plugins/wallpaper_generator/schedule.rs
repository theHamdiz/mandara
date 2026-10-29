//! The opt-in schedule, as pure decisions the plugin's entity carries out.
//!
//! * Off (the default, or the plugin switched off): no timer exists at all.
//! * On: one timer, armed for the next due time and re-armed only when that
//!   time changes. The last run is persisted
//!   (`~/.local/state/mandara/wallgen.toml`), so a restart keeps the rhythm,
//!   and a schedule that missed runs while the machine slept runs once,
//!   never a burst of catch-up runs ([`mandara_wallgen::next_due`]).
//! * Turning it on starts the clock: the first run is one interval later
//!   (Generate runs one now). Right after the shell starts, a due run waits
//!   [`STARTUP_GRACE`] so it never slows the session coming up.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use mandara_config::wallgen::WallgenSchedule;
use serde::{Deserialize, Serialize};

/// A run due at startup waits this long.
pub const STARTUP_GRACE: Duration = Duration::from_secs(45);
/// A run that falls due while another job is busy tries again this much later.
pub const BUSY_RETRY: Duration = Duration::from_secs(60);

/// What is persisted between sessions.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScheduleState {
    /// When the schedule last ran (or was turned on), Unix seconds.
    pub last_run: Option<u64>,
}

impl ScheduleState {
    pub fn path() -> PathBuf {
        mandara_config::paths::state_dir().join("wallgen.toml")
    }

    /// The saved state (empty when missing or unreadable).
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = toml::to_string(self).map_err(|e| e.to_string())?;
        mandara_config::write_atomic(path, text.as_bytes()).map_err(|e| e.to_string())
    }

    pub fn last(&self) -> Option<SystemTime> {
        self.last_run.map(|s| SystemTime::UNIX_EPOCH + Duration::from_secs(s))
    }

    pub fn set_last(&mut self, at: SystemTime) {
        self.last_run = at.duration_since(SystemTime::UNIX_EPOCH).ok().map(|d| d.as_secs());
    }
}

/// What the schedule wants right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// No timer.
    Off,
    /// The schedule was just turned on: record `now` as the last run, then
    /// wait one interval.
    Start,
    /// Wait until this moment.
    At(SystemTime),
}

/// The plan for `schedule` (`active`: the plugin is on), given the last run.
pub fn plan(schedule: &WallgenSchedule, active: bool, last: Option<SystemTime>, now: SystemTime) -> Plan {
    if !active || !schedule.enabled {
        return Plan::Off;
    }
    match last {
        None => Plan::Start,
        Some(last) => Plan::At(mandara_wallgen::next_due(Some(last), schedule.interval(), now)),
    }
}

/// What the entity does with its timer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Nothing changes.
    Keep,
    /// Drop the timer.
    Disarm,
    /// (Re)arm the timer to fire after this long.
    Arm(Duration),
}

/// The timer's bookkeeping: when it is armed for, if at all.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scheduler {
    armed: Option<SystemTime>,
    /// Runs aren't due before this (startup grace, busy retries).
    not_before: Option<SystemTime>,
}

impl Scheduler {
    /// A scheduler for a shell that started at `now`.
    pub fn starting(now: SystemTime) -> Self {
        Scheduler { armed: None, not_before: Some(now + STARTUP_GRACE) }
    }

    /// Whether a timer exists.
    pub fn armed(&self) -> Option<SystemTime> {
        self.armed
    }

    /// Holds runs off until `at` (another job is busy).
    pub fn defer_until(&mut self, at: SystemTime) {
        self.not_before = Some(at);
        self.armed = None;
    }

    /// Brings the timer in line with `plan` (after [`Plan::Start`] was
    /// recorded, pass the resulting [`Plan::At`]).
    pub fn reconcile(&mut self, plan: Plan, now: SystemTime) -> Action {
        let due = match plan {
            Plan::Off | Plan::Start => {
                let had = self.armed.take().is_some();
                return if had { Action::Disarm } else { Action::Keep };
            }
            Plan::At(due) => due,
        };
        let due = match self.not_before {
            Some(floor) if floor > due => floor,
            _ => due,
        };
        if self.armed == Some(due) {
            return Action::Keep;
        }
        self.armed = Some(due);
        Action::Arm(due.duration_since(now).unwrap_or(Duration::ZERO))
    }

    /// The timer fired: whether a run is due now (it may have been
    /// re-planned since it was armed).
    pub fn fire(&mut self, plan: Plan, now: SystemTime) -> bool {
        self.armed = None;
        let due = matches!(plan, Plan::At(at) if at <= now);
        let held = self.not_before.is_some_and(|floor| floor > now);
        if due && !held {
            self.not_before = None;
        }
        due && !held
    }
}

/// The next run as `(due, seconds from now)`, for status lines.
pub fn next_run(schedule: &WallgenSchedule, active: bool, last: Option<SystemTime>, now: SystemTime) -> Option<SystemTime> {
    match plan(schedule, active, last, now) {
        Plan::Off => None,
        Plan::Start => Some(now + schedule.interval()),
        Plan::At(at) => Some(at),
    }
}

/// Parses an interval: `6h`, `30m`, `1d`, `1w`, `1h30m` or bare minutes.
pub fn parse_every(text: &str) -> Option<u32> {
    const MIN_PER_HOUR: u64 = 60;
    const MIN_PER_DAY: u64 = 24 * MIN_PER_HOUR;
    const MIN_PER_WEEK: u64 = 7 * MIN_PER_DAY;
    let s = text.trim().to_ascii_lowercase();
    if let Ok(m) = s.parse::<u32>() {
        return (m > 0).then_some(m);
    }
    let (mut total, mut num) = (0u64, String::new());
    for c in s.chars() {
        match c {
            '0'..='9' => num.push(c),
            'm' | 'h' | 'd' | 'w' => {
                let n: u64 = num.parse().ok()?;
                num.clear();
                total += n * match c {
                    'w' => MIN_PER_WEEK,
                    'd' => MIN_PER_DAY,
                    'h' => MIN_PER_HOUR,
                    _ => 1,
                };
            }
            _ => return None,
        }
    }
    (num.is_empty() && total > 0).then(|| u32::try_from(total).ok()).flatten()
}

/// An interval for people: `6 h`, `1 d`, `45 min`, `1 h 30 min`.
pub fn every_label(minutes: u32) -> String {
    const MIN_PER_HOUR: u32 = 60;
    const MIN_PER_DAY: u32 = 24 * MIN_PER_HOUR;
    let unit = |n: u32, u: &str| format!("{n} {u}");
    if minutes >= MIN_PER_DAY && minutes.is_multiple_of(MIN_PER_DAY) {
        unit(minutes / MIN_PER_DAY, mandara_i18n::t("d"))
    } else if minutes >= MIN_PER_HOUR && minutes.is_multiple_of(MIN_PER_HOUR) {
        unit(minutes / MIN_PER_HOUR, mandara_i18n::t("h"))
    } else if minutes > MIN_PER_HOUR {
        format!("{} {}", unit(minutes / MIN_PER_HOUR, mandara_i18n::t("h")), unit(minutes % MIN_PER_HOUR, mandara_i18n::t("min")))
    } else {
        unit(minutes, mandara_i18n::t("min"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: Duration = Duration::from_secs(3600);

    fn t0() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)
    }

    fn on(every_minutes: u32) -> WallgenSchedule {
        WallgenSchedule { enabled: true, every_minutes, ..Default::default() }
    }

    #[test]
    fn off_by_default_means_no_timer() {
        let s = WallgenSchedule::default();
        assert!(!s.enabled);
        assert_eq!(plan(&s, true, None, t0()), Plan::Off);
        assert_eq!(plan(&on(60), false, Some(t0()), t0()), Plan::Off, "plugin off: no timer either");
        let mut sched = Scheduler::starting(t0());
        assert_eq!(sched.reconcile(Plan::Off, t0()), Action::Keep, "never armed: nothing to drop");
        assert_eq!(sched.armed(), None);
        assert_eq!(next_run(&s, true, None, t0()), None);
    }

    #[test]
    fn turning_on_waits_one_interval() {
        let s = on(360);
        assert_eq!(plan(&s, true, None, t0()), Plan::Start);
        // The entity records `now` as the last run, then re-plans.
        let p = plan(&s, true, Some(t0()), t0());
        assert_eq!(p, Plan::At(t0() + HOUR * 6));
        let mut sched = Scheduler::default();
        assert_eq!(sched.reconcile(p, t0()), Action::Arm(HOUR * 6));
        assert_eq!(sched.reconcile(p, t0()), Action::Keep, "armed once, not re-armed every config change");
        assert_eq!(next_run(&s, true, None, t0()), Some(t0() + HOUR * 6));
    }

    #[test]
    fn restarts_keep_the_rhythm_and_never_storm() {
        let s = on(60);
        // Last run 20 minutes before the restart: due 40 minutes in, after the grace.
        let last = t0() - Duration::from_secs(20 * 60);
        let mut sched = Scheduler::starting(t0());
        assert_eq!(sched.reconcile(plan(&s, true, Some(last), t0()), t0()), Action::Arm(Duration::from_secs(40 * 60)));
        // Off for three days: due now, but the startup grace holds it…
        let stale = t0() - HOUR * 72;
        let mut sched = Scheduler::starting(t0());
        assert_eq!(sched.reconcile(plan(&s, true, Some(stale), t0()), t0()), Action::Arm(STARTUP_GRACE));
        assert!(!sched.fire(plan(&s, true, Some(stale), t0()), t0()), "held during the grace");
        // …then exactly one run, and the next is a full interval later.
        let later = t0() + STARTUP_GRACE;
        assert_eq!(sched.reconcile(plan(&s, true, Some(stale), later), t0()), Action::Arm(STARTUP_GRACE));
        assert!(sched.fire(plan(&s, true, Some(stale), later), later));
        let p = plan(&s, true, Some(later), later);
        assert_eq!(p, Plan::At(later + HOUR));
        assert_eq!(sched.reconcile(p, later), Action::Arm(HOUR));
        assert!(!sched.fire(p, later), "not due again right away");
    }

    #[test]
    fn interval_changes_re_arm_and_turning_off_disarms() {
        let mut sched = Scheduler::default();
        let last = Some(t0());
        assert_eq!(sched.reconcile(plan(&on(60), true, last, t0()), t0()), Action::Arm(HOUR));
        assert_eq!(sched.reconcile(plan(&on(120), true, last, t0()), t0()), Action::Arm(HOUR * 2));
        assert_eq!(sched.reconcile(plan(&WallgenSchedule::default(), true, last, t0()), t0()), Action::Disarm);
        assert_eq!(sched.armed(), None);
        assert_eq!(sched.reconcile(Plan::Off, t0()), Action::Keep);
    }

    #[test]
    fn busy_runs_are_deferred() {
        let s = on(60);
        let last = Some(t0() - HOUR * 2);
        let mut sched = Scheduler::default();
        assert_eq!(sched.reconcile(plan(&s, true, last, t0()), t0()), Action::Arm(Duration::ZERO));
        sched.defer_until(t0() + BUSY_RETRY);
        assert_eq!(sched.reconcile(plan(&s, true, last, t0()), t0()), Action::Arm(BUSY_RETRY));
    }

    #[test]
    fn state_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/wallgen.toml");
        assert_eq!(ScheduleState::load(&path), ScheduleState::default());
        let mut st = ScheduleState::default();
        st.set_last(t0());
        st.save(&path).unwrap();
        assert_eq!(ScheduleState::load(&path).last(), Some(t0()));
        std::fs::write(&path, "last_run = \"garbage\"").unwrap();
        assert_eq!(ScheduleState::load(&path), ScheduleState::default());
    }

    #[test]
    fn intervals_parse_and_print() {
        assert_eq!(parse_every("6h"), Some(360));
        assert_eq!(parse_every("90"), Some(90));
        assert_eq!(parse_every("1h30m"), Some(90));
        assert_eq!(parse_every("1d"), Some(1440));
        assert_eq!(parse_every("2w"), Some(20160));
        assert_eq!(parse_every("0"), None);
        assert_eq!(parse_every("soon"), None);
        assert_eq!(parse_every("6"), Some(6));
        assert_eq!(every_label(360), "6 h");
        assert_eq!(every_label(1440), "1 d");
        assert_eq!(every_label(45), "45 min");
        assert_eq!(every_label(90), "1 h 30 min");
    }
}
