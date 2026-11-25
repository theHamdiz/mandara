//! Task progress pushed by scripts, long builds and AI agents through
//! `mandara msg progress …`: the in-memory board of tasks, value parsing
//! (`40`, `40%`, `3/10`), stall detection and the ETA from the rate so far.
//!
//! Plain data: the shell owns one [`Board`] and stamps every change with
//! the wall clock (milliseconds since the Unix epoch), so tests pass their
//! own times.

use serde::Serialize;

/// A task with no update for this long shows as stalled.
pub const STALL_AFTER_MS: i64 = 10 * 60 * 1000;
/// Finished tasks stay in the bar this long (they stay in the panel until
/// cleared).
pub const BAR_LINGER_MS: i64 = 30 * 1000;
/// No ETA before this much time has passed since the reference sample, or
/// below this fraction of the work done: early rates are noise.
const ETA_MIN_ELAPSED_MS: i64 = 2_000;
const ETA_MIN_PROGRESS: f64 = 0.01;
/// Longest id and label kept.
const MAX_ID: usize = 64;
const MAX_LABEL: usize = 160;
/// Most tasks on the board (the oldest finished ones go first).
const MAX_TASKS: usize = 64;

/// How far along a task is.
#[derive(Copy, Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Amount {
    /// `0..=1`.
    Fraction { value: f64 },
    /// `done` of `total` steps.
    Steps { done: u64, total: u64 },
}

impl Amount {
    pub fn fraction(self) -> f64 {
        match self {
            Amount::Fraction { value } => value.clamp(0.0, 1.0),
            Amount::Steps { done, total } if total > 0 => (done as f64 / total as f64).clamp(0.0, 1.0),
            Amount::Steps { .. } => 0.0,
        }
    }
}

/// Parses `40`, `40%`, `12.5%` (percent) or `3/10` (steps).
pub fn parse_amount(raw: &str) -> Result<Amount, String> {
    let s = raw.trim();
    let bad = || format!("{s:?} is not a percent (40 or 40%) or a count (3/10)");
    if let Some((done, total)) = s.split_once('/') {
        let done: u64 = done.trim().parse().map_err(|_| bad())?;
        let total: u64 = total.trim().parse().map_err(|_| bad())?;
        if total == 0 {
            return Err(format!("{s:?}: the total must be at least 1"));
        }
        return Ok(Amount::Steps { done: done.min(total), total });
    }
    let n: f64 = s.strip_suffix('%').unwrap_or(s).trim().parse().map_err(|_| bad())?;
    if !n.is_finite() || !(0.0..=100.0).contains(&n) {
        return Err(format!("{s:?}: a percent must be between 0 and 100"));
    }
    Ok(Amount::Fraction { value: n / 100.0 })
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Running,
    Done,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Task {
    pub id: String,
    pub label: String,
    pub status: Status,
    /// `None`: indeterminate (no value pushed yet).
    pub amount: Option<Amount>,
    /// The done / fail message.
    pub message: Option<String>,
    pub started_ms: i64,
    pub updated_ms: i64,
    pub finished_ms: Option<i64>,
    /// The first value pushed, the reference for the rate.
    #[serde(skip)]
    first: Option<(i64, f64)>,
}

impl Task {
    pub fn fraction(&self) -> Option<f64> {
        self.amount.map(Amount::fraction)
    }

    pub fn stalled(&self, now_ms: i64) -> bool {
        self.status == Status::Running && now_ms - self.updated_ms >= STALL_AFTER_MS
    }

    /// When the task turns stalled (running tasks).
    pub fn stalls_at(&self) -> Option<i64> {
        (self.status == Status::Running).then_some(self.updated_ms + STALL_AFTER_MS)
    }

    /// Milliseconds from start to finish (or to now).
    pub fn elapsed_ms(&self, now_ms: i64) -> i64 {
        (self.finished_ms.unwrap_or(now_ms) - self.started_ms).max(0)
    }

    /// Still shown in the bar: running, or finished under
    /// [`BAR_LINGER_MS`] ago.
    pub fn in_bar(&self, now_ms: i64) -> bool {
        self.finished_ms.is_none_or(|f| now_ms - f < BAR_LINGER_MS)
    }

    /// Time left at the rate so far, in milliseconds.
    pub fn eta_ms(&self) -> Option<i64> {
        if self.status != Status::Running {
            return None;
        }
        let f = self.fraction()?;
        eta(self.first.unwrap_or((self.started_ms, 0.0)), (self.updated_ms, f), self.started_ms)
    }
}

/// Remaining time from a reference sample `(ms, fraction)` to the latest
/// one, at the average rate between them. When the latest sample is the
/// reference itself (one value so far), the rate counts from `started_ms`
/// at zero.
pub fn eta(first: (i64, f64), latest: (i64, f64), started_ms: i64) -> Option<i64> {
    let (t1, f1) = latest;
    let (t0, f0) = if t1 > first.0 && f1 > first.1 { first } else { (started_ms, 0.0) };
    let (dt, df) = (t1 - t0, f1 - f0);
    if dt < ETA_MIN_ELAPSED_MS || df <= 0.0 || f1 < ETA_MIN_PROGRESS {
        return None;
    }
    if f1 >= 1.0 {
        return Some(0);
    }
    Some(((1.0 - f1) * dt as f64 / df).round() as i64)
}

fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max { s.to_string() } else { s.chars().take(max).collect() }
}

/// Every task, newest first.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Board {
    pub tasks: Vec<Task>,
}

impl Board {
    fn get_mut(&mut self, id: &str) -> Option<&mut Task> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }

    fn check_id(id: &str) -> Result<String, String> {
        let id = clip(id, MAX_ID);
        if id.is_empty() { Err("a task id is needed".into()) } else { Ok(id) }
    }

    /// Starts (or restarts) task `id`; it moves to the top.
    pub fn start(&mut self, id: &str, label: &str, now_ms: i64) -> Result<&Task, String> {
        let id = Self::check_id(id)?;
        let label = if label.trim().is_empty() { self.get_mut(&id).map(|t| t.label.clone()).unwrap_or_else(|| id.clone()) } else { clip(label, MAX_LABEL) };
        self.tasks.retain(|t| t.id != id);
        self.tasks.insert(0, Task { id, label, status: Status::Running, amount: None, message: None, started_ms: now_ms, updated_ms: now_ms, finished_ms: None, first: None });
        self.trim();
        Ok(&self.tasks[0])
    }

    /// Sets the value (and label) of `id`, starting it if unknown.
    pub fn set(&mut self, id: &str, amount: Amount, label: &str, now_ms: i64) -> Result<&Task, String> {
        let id = Self::check_id(id)?;
        if self.get_mut(&id).is_none_or(|t| t.status != Status::Running) {
            self.start(&id, label, now_ms)?;
        }
        let task = self.tasks.iter_mut().find(|t| t.id == id).ok_or_else(|| format!("no task {id:?}"))?;
        if !label.trim().is_empty() {
            task.label = clip(label, MAX_LABEL);
        }
        task.amount = Some(amount);
        task.updated_ms = now_ms;
        if task.first.is_none() {
            task.first = Some((now_ms, amount.fraction()));
        }
        Ok(task)
    }

    fn finish(&mut self, id: &str, status: Status, message: &str, now_ms: i64) -> Result<Task, String> {
        let id = Self::check_id(id)?;
        if self.get_mut(&id).is_none() {
            self.start(&id, "", now_ms)?;
        }
        let task = self.get_mut(&id).ok_or_else(|| format!("no task {id:?}"))?;
        task.status = status;
        task.message = (!message.trim().is_empty()).then(|| clip(message, MAX_LABEL));
        task.updated_ms = now_ms;
        task.finished_ms = Some(now_ms);
        if status == Status::Done {
            task.amount = Some(match task.amount {
                Some(Amount::Steps { total, .. }) => Amount::Steps { done: total, total },
                _ => Amount::Fraction { value: 1.0 },
            });
        }
        Ok(task.clone())
    }

    pub fn done(&mut self, id: &str, message: &str, now_ms: i64) -> Result<Task, String> {
        self.finish(id, Status::Done, message, now_ms)
    }

    pub fn fail(&mut self, id: &str, message: &str, now_ms: i64) -> Result<Task, String> {
        self.finish(id, Status::Failed, message, now_ms)
    }

    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        let before = self.tasks.len();
        self.tasks.retain(|t| t.id != id.trim());
        if self.tasks.len() == before { Err(format!("no task {:?}", id.trim())) } else { Ok(()) }
    }

    /// Drops finished tasks; returns how many.
    pub fn clear_finished(&mut self) -> usize {
        let before = self.tasks.len();
        self.tasks.retain(|t| t.status == Status::Running);
        before - self.tasks.len()
    }

    pub fn running(&self) -> impl Iterator<Item = &Task> {
        self.tasks.iter().filter(|t| t.status == Status::Running)
    }

    /// Keeps the board bounded: the oldest finished tasks go first.
    fn trim(&mut self) {
        while self.tasks.len() > MAX_TASKS {
            match self.tasks.iter().rposition(|t| t.status != Status::Running) {
                Some(i) => {
                    self.tasks.remove(i);
                }
                None => {
                    self.tasks.pop();
                }
            }
        }
    }

    /// The next moment something changes on its own (a task stalls or a
    /// finished task leaves the bar), after `now_ms`.
    pub fn next_change(&self, now_ms: i64) -> Option<i64> {
        self.tasks
            .iter()
            .filter_map(|t| match t.finished_ms {
                Some(f) => Some(f + BAR_LINGER_MS),
                None => t.stalls_at(),
            })
            .filter(|at| *at > now_ms)
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_parse_as_percent_or_steps() {
        assert_eq!(parse_amount("40"), Ok(Amount::Fraction { value: 0.4 }));
        assert_eq!(parse_amount("40%"), Ok(Amount::Fraction { value: 0.4 }));
        assert_eq!(parse_amount(" 12.5 % "), Ok(Amount::Fraction { value: 0.125 }));
        assert_eq!(parse_amount("3/10"), Ok(Amount::Steps { done: 3, total: 10 }));
        assert_eq!(parse_amount("12/10"), Ok(Amount::Steps { done: 10, total: 10 }), "clamped to the total");
        assert!(parse_amount("3/0").is_err());
        assert!(parse_amount("101").is_err());
        assert!(parse_amount("-5").is_err());
        assert!(parse_amount("half").is_err());
        assert!(parse_amount("NaN").is_err());
        assert!((Amount::Steps { done: 1, total: 4 }.fraction() - 0.25).abs() < 1e-9);
    }

    #[test]
    fn eta_follows_the_rate_so_far() {
        // One value: 25% after 10 s from the start → 30 s left.
        assert_eq!(eta((10_000, 0.25), (10_000, 0.25), 0), Some(30_000));
        // Two samples: 20% → 60% in 8 s (5%/s) → 40% left = 8 s.
        assert_eq!(eta((2_000, 0.2), (10_000, 0.6), 0), Some(8_000));
        // Too early or no progress: no estimate.
        assert_eq!(eta((500, 0.5), (500, 0.5), 0), None);
        assert_eq!(eta((2_000, 0.4), (10_000, 0.4), 0), Some(15_000), "no gain since the first sample: rate from the start");
        assert_eq!(eta((0, 0.0), (10_000, 0.0), 0), None);
        assert_eq!(eta((1_000, 0.1), (5_000, 1.0), 0), Some(0));
    }

    #[test]
    fn board_tracks_tasks_through_their_life() {
        let mut b = Board::default();
        b.start("build", "Building Mandara", 0).unwrap();
        assert_eq!(b.tasks[0].fraction(), None, "indeterminate until a value arrives");
        b.set("build", parse_amount("3/10").unwrap(), "", 4_000).unwrap();
        b.set("build", parse_amount("6/10").unwrap(), "Linking", 8_000).unwrap();
        let t = &b.tasks[0];
        assert_eq!(t.label, "Linking");
        assert_eq!(t.eta_ms(), Some(5_333), "30% in 4 s → 40% in 5.3 s");
        assert!(!t.stalled(8_000 + STALL_AFTER_MS - 1) && t.stalled(8_000 + STALL_AFTER_MS));
        assert_eq!(b.next_change(9_000), Some(8_000 + STALL_AFTER_MS));
        // Setting an unknown id starts it.
        b.set("agent", parse_amount("50%").unwrap(), "Refactoring", 9_000).unwrap();
        assert_eq!(b.tasks[0].id, "agent");
        let done = b.done("build", "All green", 20_000).unwrap();
        assert_eq!((done.status, done.fraction(), done.message.as_deref()), (Status::Done, Some(1.0), Some("All green")));
        assert!(b.tasks.iter().find(|t| t.id == "build").unwrap().in_bar(20_000 + BAR_LINGER_MS - 1));
        assert!(!b.tasks.iter().find(|t| t.id == "build").unwrap().in_bar(20_000 + BAR_LINGER_MS));
        b.fail("agent", "Tests failed", 21_000).unwrap();
        assert_eq!(b.running().count(), 0);
        assert_eq!(b.clear_finished(), 2);
        assert!(b.remove("build").is_err());
        assert!(b.start("  ", "x", 0).is_err());
    }

    #[test]
    fn restarting_keeps_the_label_and_the_board_stays_bounded() {
        let mut b = Board::default();
        b.start("job", "Nightly", 0).unwrap();
        b.done("job", "", 10).unwrap();
        b.start("job", "", 20).unwrap();
        assert_eq!((b.tasks[0].label.as_str(), b.tasks[0].status), ("Nightly", Status::Running));
        for i in 0..MAX_TASKS + 5 {
            b.start(&format!("t{i}"), "", i as i64).unwrap();
        }
        assert_eq!(b.tasks.len(), MAX_TASKS);
    }
}
