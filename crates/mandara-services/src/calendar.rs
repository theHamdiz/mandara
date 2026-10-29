//! Calendar events from iCalendar (RFC 5545) files: a small pure-Rust
//! parser for `VEVENT`s (with `VALARM` triggers), recurrence expansion for
//! the common `RRULE`s (daily / weekly with `BYDAY` / monthly by day /
//! yearly, `INTERVAL`, `COUNT`, `UNTIL`, `EXDATE`, `RECURRENCE-ID`
//! overrides) and loading from `https://` / `webcal://` subscriptions or
//! local `.ics` files (fetched subscriptions are cached for offline use).
//!
//! Not supported (rare in practice): `BYSETPOS`, `BYWEEKNO`, `BYYEARDAY`,
//! hourly / minutely frequencies, `RDATE`, `VTODO`.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Datelike, Days, Local, Months, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc, Weekday};

/// Hard cap on expanded instances per event (guards absurd rules).
const MAX_INSTANCES: usize = 5000;
/// Largest ICS download accepted, bytes.
const MAX_ICS_BYTES: u64 = 16 * 1024 * 1024;
/// Download timeout.
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
/// Seconds per minute / hour / day / week (duration parsing).
const S_PER_MIN: i64 = 60;
const S_PER_HOUR: i64 = 3600;
const S_PER_DAY: i64 = 86_400;
const S_PER_WEEK: i64 = 7 * S_PER_DAY;

/// When an event starts / ends.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum When {
    /// A whole day (`VALUE=DATE`); ends are exclusive.
    Date(NaiveDate),
    /// An absolute instant.
    Instant(DateTime<Utc>),
}

impl When {
    /// The local start of this moment (midnight for dates).
    pub fn local(self) -> DateTime<Local> {
        match self {
            When::Instant(t) => t.with_timezone(&Local),
            When::Date(d) => local_midnight(d),
        }
    }

    fn shift(self, by: Step) -> Option<When> {
        Some(match self {
            When::Date(d) => When::Date(by.apply_date(d)?),
            // Recurrences keep the local wall-clock time across DST.
            When::Instant(t) => {
                let local = t.with_timezone(&Local).naive_local();
                let next = NaiveDateTime::new(by.apply_date(local.date())?, local.time());
                When::Instant(Local.from_local_datetime(&next).earliest()?.with_timezone(&Utc))
            }
        })
    }
}

fn local_midnight(d: NaiveDate) -> DateTime<Local> {
    Local.from_local_datetime(&d.and_time(NaiveTime::MIN)).earliest().unwrap_or_else(|| Utc.from_utc_datetime(&d.and_time(NaiveTime::MIN)).with_timezone(&Local))
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum Step {
    Days(u64),
    Months(u32),
}

impl Step {
    fn apply_date(self, d: NaiveDate) -> Option<NaiveDate> {
        match self {
            Step::Days(n) => d.checked_add_days(Days::new(n)),
            Step::Months(n) => d.checked_add_months(Months::new(n)),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

/// The supported subset of an `RRULE`.
#[derive(Clone, Debug, PartialEq)]
pub struct RRule {
    pub freq: Freq,
    pub interval: u32,
    pub count: Option<u32>,
    pub until: Option<When>,
    /// Weekly `BYDAY` (plain weekdays, no ordinals).
    pub by_day: Vec<Weekday>,
    /// Monthly `BYMONTHDAY` (positive days only).
    pub by_month_day: Vec<u32>,
}

/// One parsed `VEVENT`.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub uid: String,
    pub summary: String,
    pub location: String,
    pub description: String,
    pub start: When,
    pub end: When,
    pub rrule: Option<RRule>,
    pub exdates: Vec<When>,
    /// Set on an override of one instance of a recurring event.
    pub recurrence_id: Option<When>,
    /// Alarm lead times before the start (from `VALARM` relative triggers).
    pub alarms: Vec<Duration>,
}

/// One concrete occurrence in a time window.
#[derive(Clone, Debug, PartialEq)]
pub struct Occurrence {
    pub uid: String,
    pub summary: String,
    pub location: String,
    pub start: DateTime<Local>,
    pub end: DateTime<Local>,
    pub all_day: bool,
    pub alarms: Vec<Duration>,
    /// Index of the calendar source it came from.
    pub source: usize,
}

// ── Parsing ──────────────────────────────────────────────────────────────

/// Unfolds RFC 5545 content lines (a line starting with a space or tab
/// continues the previous one).
fn unfold(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = line.strip_prefix(' ').or_else(|| line.strip_prefix('\t')) {
            if let Some(last) = out.last_mut() {
                last.push_str(rest);
                continue;
            }
        }
        if !line.is_empty() {
            out.push(line.to_string());
        }
    }
    out
}

/// A content line: `NAME;PARAM=V;…:value`.
struct Line<'a> {
    name: String,
    params: Vec<(String, &'a str)>,
    value: &'a str,
}

fn split_line(line: &str) -> Option<Line<'_>> {
    // The value starts at the first ':' outside a quoted parameter value.
    let mut quoted = false;
    let colon = line.char_indices().find(|(_, c)| {
        if *c == '"' {
            quoted = !quoted;
        }
        *c == ':' && !quoted
    })?;
    let (head, value) = (&line[..colon.0], &line[colon.0 + 1..]);
    let mut parts = head.split(';');
    let name = parts.next()?.trim().to_ascii_uppercase();
    let params = parts.filter_map(|p| p.split_once('=')).map(|(k, v)| (k.trim().to_ascii_uppercase(), v.trim_matches('"'))).collect();
    Some(Line { name, params, value })
}

impl Line<'_> {
    fn param(&self, key: &str) -> Option<&str> {
        self.params.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }
}

/// Unescapes a TEXT value (`\n`, `\,`, `\;`, `\\`).
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n' | 'N') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Parses a DATE / DATE-TIME value with its `VALUE` / `TZID` parameters.
fn parse_when(value: &str, value_type: Option<&str>, tzid: Option<&str>) -> Option<When> {
    let value = value.trim();
    if value_type.is_some_and(|v| v.eq_ignore_ascii_case("DATE")) || value.len() == 8 {
        return NaiveDate::parse_from_str(value, "%Y%m%d").ok().map(When::Date);
    }
    let (body, utc) = match value.strip_suffix('Z') {
        Some(b) => (b, true),
        None => (value, false),
    };
    let naive = NaiveDateTime::parse_from_str(body, "%Y%m%dT%H%M%S").ok()?;
    if utc {
        return Some(When::Instant(Utc.from_utc_datetime(&naive)));
    }
    if let Some(tz) = tzid.and_then(|t| t.trim_start_matches('/').parse::<chrono_tz::Tz>().ok()) {
        return tz.from_local_datetime(&naive).earliest().map(|t| When::Instant(t.with_timezone(&Utc)));
    }
    // Floating time (or an unknown TZID): the viewer's local time.
    Local.from_local_datetime(&naive).earliest().map(|t| When::Instant(t.with_timezone(&Utc)))
}

/// Parses an RFC 5545 DURATION (`-PT15M`, `P1D`, `PT1H30M`, `P2W`) in seconds.
pub fn parse_duration(s: &str) -> Option<i64> {
    let s = s.trim();
    let (sign, rest) = match s.strip_prefix('-') {
        Some(r) => (-1, r),
        None => (1, s.strip_prefix('+').unwrap_or(s)),
    };
    let rest = rest.strip_prefix('P')?;
    let mut total = 0i64;
    let mut number = String::new();
    let mut in_time = false;
    for c in rest.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => number.push(c),
            unit => {
                let n: i64 = number.parse().ok()?;
                number.clear();
                total += n * match (unit, in_time) {
                    ('W', false) => S_PER_WEEK,
                    ('D', false) => S_PER_DAY,
                    ('H', true) => S_PER_HOUR,
                    ('M', true) => S_PER_MIN,
                    ('S', true) => 1,
                    _ => return None,
                };
            }
        }
    }
    number.is_empty().then_some(sign * total)
}

fn parse_weekday(s: &str) -> Option<Weekday> {
    // Ordinal prefixes (`1MO`, `-1FR`) are not supported: skip those.
    Some(match s {
        "MO" => Weekday::Mon,
        "TU" => Weekday::Tue,
        "WE" => Weekday::Wed,
        "TH" => Weekday::Thu,
        "FR" => Weekday::Fri,
        "SA" => Weekday::Sat,
        "SU" => Weekday::Sun,
        _ => return None,
    })
}

fn parse_rrule(value: &str) -> Option<RRule> {
    let mut rule = RRule { freq: Freq::Daily, interval: 1, count: None, until: None, by_day: Vec::new(), by_month_day: Vec::new() };
    let mut freq = None;
    for part in value.split(';') {
        let (k, v) = part.split_once('=')?;
        match k.to_ascii_uppercase().as_str() {
            "FREQ" => {
                freq = Some(match v {
                    "DAILY" => Freq::Daily,
                    "WEEKLY" => Freq::Weekly,
                    "MONTHLY" => Freq::Monthly,
                    "YEARLY" => Freq::Yearly,
                    _ => return None,
                })
            }
            "INTERVAL" => rule.interval = v.parse().ok().filter(|n| *n > 0)?,
            "COUNT" => rule.count = v.parse().ok(),
            "UNTIL" => rule.until = parse_when(v, None, None),
            "BYDAY" => rule.by_day = v.split(',').filter_map(parse_weekday).collect(),
            "BYMONTHDAY" => rule.by_month_day = v.split(',').filter_map(|d| d.parse().ok()).filter(|d| (1..=31).contains(d)).collect(),
            _ => {}
        }
    }
    rule.freq = freq?;
    Some(rule)
}

/// Parses every non-cancelled `VEVENT` of an iCalendar document.
pub fn parse_ics(text: &str) -> Vec<Event> {
    let mut events = Vec::new();
    let mut current: Option<EventBuilder> = None;
    let mut in_alarm = false;
    for raw in unfold(text) {
        let Some(line) = split_line(&raw) else { continue };
        match (line.name.as_str(), line.value.trim().to_ascii_uppercase().as_str()) {
            ("BEGIN", "VEVENT") => current = Some(EventBuilder::default()),
            ("END", "VEVENT") => {
                if let Some(event) = current.take().and_then(EventBuilder::build) {
                    events.push(event);
                }
            }
            ("BEGIN", "VALARM") => in_alarm = true,
            ("END", "VALARM") => in_alarm = false,
            _ => {
                let Some(ev) = current.as_mut() else { continue };
                if in_alarm {
                    let relative_to_end = line.param("RELATED").is_some_and(|r| r.eq_ignore_ascii_case("END"));
                    if line.name == "TRIGGER" && !relative_to_end && line.param("VALUE").is_none_or(|v| v.eq_ignore_ascii_case("DURATION")) {
                        // Only alarms before (or at) the start.
                        if let Some(secs) = parse_duration(line.value).filter(|s| *s <= 0) {
                            ev.alarms.push(Duration::from_secs(secs.unsigned_abs()));
                        }
                    }
                    continue;
                }
                let (ty, tz) = (line.param("VALUE"), line.param("TZID"));
                match line.name.as_str() {
                    "UID" => ev.uid = line.value.trim().to_string(),
                    "SUMMARY" => ev.summary = unescape(line.value),
                    "LOCATION" => ev.location = unescape(line.value),
                    "DESCRIPTION" => ev.description = unescape(line.value),
                    "DTSTART" => ev.start = parse_when(line.value, ty, tz),
                    "DTEND" => ev.end = parse_when(line.value, ty, tz),
                    "DURATION" => ev.duration = parse_duration(line.value),
                    "RRULE" => ev.rrule = parse_rrule(line.value),
                    "EXDATE" => ev.exdates.extend(line.value.split(',').filter_map(|v| parse_when(v, ty, tz))),
                    "RECURRENCE-ID" => ev.recurrence_id = parse_when(line.value, ty, tz),
                    "STATUS" => ev.cancelled = line.value.trim().eq_ignore_ascii_case("CANCELLED"),
                    _ => {}
                }
            }
        }
    }
    events
}

#[derive(Default)]
struct EventBuilder {
    uid: String,
    summary: String,
    location: String,
    description: String,
    start: Option<When>,
    end: Option<When>,
    duration: Option<i64>,
    rrule: Option<RRule>,
    exdates: Vec<When>,
    recurrence_id: Option<When>,
    alarms: Vec<Duration>,
    cancelled: bool,
}

impl EventBuilder {
    fn build(self) -> Option<Event> {
        let start = self.start?;
        let end = match (self.end, self.duration, start) {
            (Some(end), _, _) => end,
            (None, Some(secs), When::Instant(t)) => When::Instant(t + chrono::Duration::seconds(secs)),
            (None, Some(secs), When::Date(d)) => When::Date(d.checked_add_days(Days::new(u64::try_from(secs / S_PER_DAY).unwrap_or(1).max(1)))?),
            // RFC 5545: a DATE start without end lasts one day; a
            // DATE-TIME start without end is instantaneous.
            (None, None, When::Date(d)) => When::Date(d.succ_opt()?),
            (None, None, instant) => instant,
        };
        // A cancelled override still hides its instance (kept with no title).
        if self.cancelled && self.recurrence_id.is_none() {
            return None;
        }
        Some(Event {
            uid: self.uid,
            summary: if self.cancelled { String::new() } else { self.summary },
            location: self.location,
            description: self.description,
            start,
            end,
            rrule: self.rrule,
            exdates: self.exdates,
            recurrence_id: self.recurrence_id,
            alarms: self.alarms,
        })
    }
}

// ── Recurrence expansion ─────────────────────────────────────────────────

fn duration_between(a: When, b: When) -> chrono::Duration {
    b.local().signed_duration_since(a.local())
}

/// Start times of `event` (recurring or not) before `to`, in order.
fn instances(event: &Event, to: DateTime<Local>) -> Vec<When> {
    let Some(rule) = &event.rrule else { return vec![event.start] };
    let mut out = Vec::new();
    // UNTIL is inclusive; a DATE until covers that whole day.
    let before_end = |w: When| match rule.until {
        None => true,
        Some(When::Date(d)) => w.local().date_naive() <= d,
        Some(u) => w.local() <= u.local(),
    };
    let mut emitted = 0u32;
    let mut push = |w: When, out: &mut Vec<When>| -> bool {
        if rule.count.is_some_and(|c| emitted >= c) || !before_end(w) || w.local() >= to || out.len() >= MAX_INSTANCES {
            return false;
        }
        emitted += 1;
        out.push(w);
        true
    };
    match rule.freq {
        Freq::Weekly if !rule.by_day.is_empty() => {
            // Walk weeks from the start's week; emit chosen weekdays in order.
            let start_date = event.start.local().date_naive();
            let week0 = start_date - chrono::Duration::days(i64::from(start_date.weekday().num_days_from_monday()));
            let mut days: Vec<u32> = rule.by_day.iter().map(|d| d.num_days_from_monday()).collect();
            days.sort_unstable();
            days.dedup();
            let mut week = 0u64;
            'weeks: loop {
                for d in &days {
                    let offset = week * 7 * u64::from(rule.interval) + u64::from(*d);
                    let Some(date) = week0.checked_add_days(Days::new(offset)) else { break 'weeks };
                    if date < start_date {
                        continue;
                    }
                    let delta = u64::try_from((date - start_date).num_days()).unwrap_or(0);
                    let Some(w) = event.start.shift(Step::Days(delta)) else { break 'weeks };
                    if !push(w, &mut out) {
                        break 'weeks;
                    }
                }
                week += 1;
            }
        }
        Freq::Monthly if !rule.by_month_day.is_empty() => {
            let start_date = event.start.local().date_naive();
            let first = start_date.with_day(1).unwrap_or(start_date);
            let mut month = 0u32;
            'months: loop {
                let Some(base) = first.checked_add_months(Months::new(month * rule.interval)) else { break };
                let mut days = rule.by_month_day.clone();
                days.sort_unstable();
                for d in days {
                    // Months without that day are skipped (RFC 5545).
                    let Some(date) = base.with_day(d) else { continue };
                    if date < start_date {
                        continue;
                    }
                    let delta = u64::try_from((date - start_date).num_days()).unwrap_or(0);
                    let Some(w) = event.start.shift(Step::Days(delta)) else { break 'months };
                    if !push(w, &mut out) {
                        break 'months;
                    }
                }
                month += 1;
                if base.year() > to.year() + 1 {
                    break;
                }
            }
        }
        freq => {
            let mut n = 0u32;
            loop {
                let step = match freq {
                    Freq::Daily => Step::Days(u64::from(n) * u64::from(rule.interval)),
                    Freq::Weekly => Step::Days(u64::from(n) * 7 * u64::from(rule.interval)),
                    Freq::Monthly => Step::Months(n * rule.interval),
                    Freq::Yearly => Step::Months(n * rule.interval * 12),
                };
                // Monthly / yearly from the original start (31st → skip
                // short months rather than drift).
                let Some(w) = event.start.shift(step) else { break };
                let drifted = matches!(freq, Freq::Monthly | Freq::Yearly) && w.local().day() != event.start.local().day();
                if !drifted && !push(w, &mut out) {
                    break;
                }
                n += 1;
                if out.len() >= MAX_INSTANCES || n as usize > MAX_INSTANCES * 2 {
                    break;
                }
            }
        }
    }
    out
}

/// Every occurrence overlapping `[from, to)`, sorted by start. Overrides
/// (`RECURRENCE-ID`) replace the instance they name; `EXDATE`s are removed.
pub fn occurrences(events: &[(usize, Event)], from: DateTime<Local>, to: DateTime<Local>) -> Vec<Occurrence> {
    let overridden: HashSet<(String, DateTime<Local>)> = events.iter().filter_map(|(_, e)| e.recurrence_id.map(|r| (e.uid.clone(), r.local()))).collect();
    let mut out = Vec::new();
    for (source, event) in events {
        let length = duration_between(event.start, event.end);
        for start in instances(event, to) {
            let local = start.local();
            if event.recurrence_id.is_none() && (event.exdates.iter().any(|x| x.local() == local) || overridden.contains(&(event.uid.clone(), local))) {
                continue;
            }
            if event.summary.is_empty() && event.recurrence_id.is_some() {
                continue; // a cancelled instance
            }
            let end = local + length;
            if end <= from && local < from || local >= to {
                continue;
            }
            out.push(Occurrence {
                uid: event.uid.clone(),
                summary: event.summary.clone(),
                location: event.location.clone(),
                start: local,
                end,
                all_day: matches!(event.start, When::Date(_)),
                alarms: event.alarms.clone(),
                source: *source,
            });
        }
    }
    out.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| a.summary.cmp(&b.summary)));
    // The same event in two subscribed calendars shows once.
    out.dedup_by(|a, b| a.uid == b.uid && !a.uid.is_empty() && a.start == b.start);
    out
}

impl Occurrence {
    /// JSON for `mandara msg calendar …` (RFC 3339 times).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "uid": self.uid,
            "summary": self.summary,
            "location": self.location,
            "start": self.start.to_rfc3339(),
            "end": self.end.to_rfc3339(),
            "all_day": self.all_day,
            "source": self.source,
        })
    }
}

/// Reminders due in `(after, until]`: `(occurrence, lead time)` pairs.
/// `fallback` applies to events without alarms (or when `honour_alarms`
/// is off); all-day events remind at `fallback` before midnight only when
/// they define an alarm.
pub fn due_reminders(occ: &[Occurrence], after: DateTime<Local>, until: DateTime<Local>, fallback: Duration, honour_alarms: bool) -> Vec<(Occurrence, Duration)> {
    let mut out = Vec::new();
    for o in occ {
        let leads: Vec<Duration> = if honour_alarms && !o.alarms.is_empty() {
            o.alarms.clone()
        } else if o.all_day {
            Vec::new()
        } else {
            vec![fallback]
        };
        for lead in leads {
            let Ok(lead_c) = chrono::Duration::from_std(lead) else { continue };
            let at = o.start - lead_c;
            if at > after && at <= until {
                out.push((o.clone(), lead));
            }
        }
    }
    out
}

// ── Loading ──────────────────────────────────────────────────────────────

/// Where a fetched subscription is cached.
fn cache_path(url: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    mandara_config::paths::cache_dir().join("calendars").join(format!("{:016x}.ics", h.finish()))
}

/// Expands `~/` in a local path.
fn expand(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
        None => PathBuf::from(path),
    }
}

/// Whether `location` is fetched over the network.
pub fn is_remote(location: &str) -> bool {
    let l = location.trim().to_ascii_lowercase();
    l.starts_with("https://") || l.starts_with("http://") || l.starts_with("webcal://") || l.starts_with("webcals://")
}

/// The last downloaded copy of a subscription (offline mode).
pub fn load_cached(location: &str) -> Option<String> {
    let location = location.trim();
    let url = if let Some(rest) = location.strip_prefix("webcals://").or_else(|| location.strip_prefix("webcal://")) { format!("https://{rest}") } else { location.to_string() };
    std::fs::read_to_string(cache_path(&url)).ok()
}

/// Reads one calendar: downloads a subscription (falling back to the last
/// cached copy when offline) or reads a local file (a folder is read as a
/// vdir: every `.ics` inside).
pub fn load(location: &str) -> Result<String, String> {
    let location = location.trim();
    if is_remote(location) {
        let url = if let Some(rest) = location.strip_prefix("webcals://").or_else(|| location.strip_prefix("webcal://")) { format!("https://{rest}") } else { location.to_string() };
        let cache = cache_path(&url);
        if crate::offline::is_offline() {
            return std::fs::read_to_string(&cache).map_err(|_| crate::offline::blocked());
        }
        let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(FETCH_TIMEOUT)).build().into();
        let fetched = agent.get(&url).call().map_err(|e| e.to_string()).and_then(|mut r| r.body_mut().with_config().limit(MAX_ICS_BYTES).read_to_string().map_err(|e| e.to_string()));
        return match fetched {
            Ok(text) => {
                if let Some(dir) = cache.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(&cache, &text);
                Ok(text)
            }
            Err(err) => std::fs::read_to_string(&cache).map_err(|_| format!("{url}: {err}")),
        };
    }
    let path = expand(location);
    if path.is_dir() {
        let mut text = String::new();
        let entries = std::fs::read_dir(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ics")) {
                text.push_str(&std::fs::read_to_string(&p).unwrap_or_default());
                text.push('\n');
            }
        }
        return Ok(text);
    }
    std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, m, d, h, min, 0).single().unwrap()
    }

    const SAMPLE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:standup@x\r\nSUMMARY:Stand-up\\, team\r\nDTSTART:20261026T090000\r\nDTEND:20261026T091500\r\nRRULE:FREQ=WEEKLY;BYDAY=MO,WE,FR;COUNT=6\r\nEXDATE:20261028T090000\r\nBEGIN:VALARM\r\nTRIGGER:-PT5M\r\nACTION:DISPLAY\r\nEND:VALARM\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:standup@x\r\nRECURRENCE-ID:20261030T090000\r\nSUMMARY:Stand-up (moved)\r\nDTSTART:20261030T100000\r\nDTEND:20261030T101500\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:holiday@x\r\nSUMMARY:Holi\r\n day\r\nDTSTART;VALUE=DATE:20261029\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:gone@x\r\nSTATUS:CANCELLED\r\nSUMMARY:Gone\r\nDTSTART:20261029T120000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn parses_folding_escapes_dates_and_alarms() {
        let events = parse_ics(SAMPLE);
        assert_eq!(events.len(), 3, "cancelled event dropped");
        assert_eq!(events[0].summary, "Stand-up, team");
        assert_eq!(events[0].alarms, vec![Duration::from_secs(300)]);
        assert_eq!(events[2].summary, "Holiday", "folded line joined");
        assert_eq!(events[2].start, When::Date(NaiveDate::from_ymd_opt(2026, 10, 29).unwrap()));
        assert_eq!(events[2].end, When::Date(NaiveDate::from_ymd_opt(2026, 10, 30).unwrap()), "all-day default length");
    }

    #[test]
    fn expands_weekly_byday_with_exdate_and_override() {
        let events: Vec<(usize, Event)> = parse_ics(SAMPLE).into_iter().map(|e| (0, e)).collect();
        let occ = occurrences(&events, local(2026, 10, 26, 0, 0), local(2026, 11, 9, 0, 0));
        let standups: Vec<_> = occ.iter().filter(|o| o.uid == "standup@x").map(|o| (o.start.day(), o.start.format("%H:%M").to_string(), o.summary.clone())).collect();
        // COUNT=6 → Mon 26, Wed 28 (excluded), Fri 30 (moved to 10:00), Mon 2, Wed 4, Fri 6.
        assert_eq!(
            standups,
            vec![
                (26, "09:00".into(), "Stand-up, team".into()),
                (30, "10:00".into(), "Stand-up (moved)".into()),
                (2, "09:00".into(), "Stand-up, team".into()),
                (4, "09:00".into(), "Stand-up, team".into()),
                (6, "09:00".into(), "Stand-up, team".into()),
            ]
        );
        assert!(occ.iter().any(|o| o.all_day && o.summary == "Holiday"));
    }

    #[test]
    fn monthly_yearly_until_and_durations() {
        let ics = "BEGIN:VEVENT\nUID:rent\nSUMMARY:Rent\nDTSTART;VALUE=DATE:20260131\nRRULE:FREQ=MONTHLY;UNTIL=20260601\nEND:VEVENT\nBEGIN:VEVENT\nUID:bday\nSUMMARY:Birthday\nDTSTART;VALUE=DATE:20200229\nRRULE:FREQ=YEARLY\nEND:VEVENT\nBEGIN:VEVENT\nUID:pay\nSUMMARY:Pay\nDTSTART:20260115T080000\nDURATION:PT1H30M\nRRULE:FREQ=MONTHLY;BYMONTHDAY=15,30;COUNT=4\nEND:VEVENT\n";
        let events: Vec<(usize, Event)> = parse_ics(ics).into_iter().map(|e| (1, e)).collect();
        let occ = occurrences(&events, local(2026, 1, 1, 0, 0), local(2029, 1, 1, 0, 0));
        let rent: Vec<u32> = occ.iter().filter(|o| o.uid == "rent").map(|o| o.start.month()).collect();
        assert_eq!(rent, vec![1, 3, 5], "months without a 31st are skipped; UNTIL stops in June");
        let bday: Vec<i32> = occ.iter().filter(|o| o.uid == "bday").map(|o| o.start.year()).collect();
        assert_eq!(bday, vec![2028], "29 Feb only in leap years");
        let pay: Vec<(u32, u32)> = occ.iter().filter(|o| o.uid == "pay").map(|o| (o.start.month(), o.start.day())).collect();
        assert_eq!(pay, vec![(1, 15), (1, 30), (2, 15), (3, 15)], "no 30 Feb");
        let first = occ.iter().find(|o| o.uid == "pay").unwrap();
        assert_eq!((first.end - first.start).num_minutes(), 90);
        assert!(occ.iter().all(|o| o.source == 1));
        assert_eq!(parse_duration("-P1DT2H"), Some(-(S_PER_DAY + 2 * S_PER_HOUR)));
        assert_eq!(parse_duration("P2W"), Some(2 * S_PER_WEEK));
        assert_eq!(parse_duration("PT"), Some(0));
        assert_eq!(parse_duration("1H"), None);
    }

    #[test]
    fn timezones_and_reminders() {
        let ics = "BEGIN:VEVENT\nUID:call\nSUMMARY:Call\nDTSTART;TZID=Europe/London:20260701T150000\nDTEND;TZID=Europe/London:20260701T160000\nEND:VEVENT\n";
        let e = &parse_ics(ics)[0];
        // 15:00 BST = 14:00 UTC.
        assert_eq!(e.start, When::Instant(Utc.with_ymd_and_hms(2026, 7, 1, 14, 0, 0).unwrap()));
        let events = vec![(0, e.clone())];
        let occ = occurrences(&events, local(2026, 7, 1, 0, 0) - chrono::Duration::days(1), local(2026, 7, 3, 0, 0));
        let start = occ[0].start;
        let ten = Duration::from_secs(600);
        let due = due_reminders(&occ, start - chrono::Duration::minutes(11), start - chrono::Duration::minutes(9), ten, true);
        assert_eq!(due.len(), 1);
        assert!(due_reminders(&occ, start - chrono::Duration::minutes(9), start, ten, true).is_empty(), "never twice");
    }

    #[test]
    fn remote_detection_and_local_files() {
        assert!(is_remote("webcal://example.com/a.ics") && is_remote(" HTTPS://x/y.ics"));
        assert!(!is_remote("~/cal.ics"));
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.ics"), "BEGIN:VEVENT\nUID:a\nDTSTART:20260101T100000Z\nEND:VEVENT\n").unwrap();
        std::fs::write(dir.path().join("b.ics"), "BEGIN:VEVENT\nUID:b\nDTSTART:20260102T100000Z\nEND:VEVENT\n").unwrap();
        let text = load(dir.path().to_str().unwrap()).unwrap();
        assert_eq!(parse_ics(&text).len(), 2, "a folder is read as a vdir");
        assert!(load("/nonexistent/x.ics").is_err());
    }
}
