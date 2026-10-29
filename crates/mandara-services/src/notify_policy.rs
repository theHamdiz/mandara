//! The notification server's decisions, kept pure so they're unit-tested:
//! per-app rules (`[[notifications.rules]]`), popup / history / sound /
//! do-not-disturb outcomes, popup lifetimes, quiet hours, and the
//! `body-markup` parser (bold, italic, underline, links).

use chrono::NaiveTime;
use mandara_config::alerts::{NotificationRule, NotificationsConfig, UrgencyLevel};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use crate::notifications::Urgency;

/// Compiled regexes are capped (a pathological pattern can't eat memory).
const REGEX_SIZE_LIMIT: usize = 1 << 20;
const MS_PER_S: f32 = 1000.0;

impl From<UrgencyLevel> for Urgency {
    fn from(u: UrgencyLevel) -> Self {
        match u {
            UrgencyLevel::Low => Urgency::Low,
            UrgencyLevel::Normal => Urgency::Normal,
            UrgencyLevel::Critical => Urgency::Critical,
        }
    }
}

impl From<Urgency> for UrgencyLevel {
    fn from(u: Urgency) -> Self {
        match u {
            Urgency::Low => UrgencyLevel::Low,
            Urgency::Normal => UrgencyLevel::Normal,
            Urgency::Critical => UrgencyLevel::Critical,
        }
    }
}

/// What an incoming notification looks like to the rules.
#[derive(Clone, Debug, Default)]
pub struct Incoming<'a> {
    pub app: &'a str,
    pub desktop_entry: &'a str,
    pub summary: &'a str,
    pub body: &'a str,
    pub category: &'a str,
    pub urgency: Urgency,
    /// Requested expiry, ms (`-1` server default, `0` never).
    pub expire_timeout: i32,
    /// The `transient` hint: never keep it in the history.
    pub transient: bool,
    /// The `suppress-sound` hint.
    pub suppress_sound: bool,
}

/// The outcome for one notification.
#[derive(Clone, Debug, PartialEq)]
pub struct Decision {
    /// Dropped entirely.
    pub block: bool,
    /// Pops up now.
    pub popup: bool,
    /// Kept in the history.
    pub history: bool,
    pub sound: bool,
    pub urgency: Urgency,
    /// Popup lifetime, ms (0 = until dismissed).
    pub display_ms: u32,
    /// The index of the matching rule.
    pub rule: Option<usize>,
}

/// A pattern: a case-insensitive regex, or plain text when it isn't one.
#[derive(Debug)]
enum Pattern {
    Any,
    Regex(Regex),
    Text(String),
}

impl Pattern {
    fn new(src: &str) -> Self {
        let src = src.trim();
        if src.is_empty() {
            return Pattern::Any;
        }
        match RegexBuilder::new(src).case_insensitive(true).size_limit(REGEX_SIZE_LIMIT).build() {
            Ok(re) => Pattern::Regex(re),
            Err(_) => Pattern::Text(src.to_lowercase()),
        }
    }

    fn matches(&self, s: &str) -> bool {
        match self {
            Pattern::Any => true,
            Pattern::Regex(re) => re.is_match(s),
            Pattern::Text(t) => s.to_lowercase().contains(t),
        }
    }
}

#[derive(Debug)]
struct Compiled {
    app: Pattern,
    summary: Pattern,
    body: Pattern,
    category: Pattern,
}

/// The config with its rules compiled once.
#[derive(Debug)]
pub struct Policy {
    pub cfg: NotificationsConfig,
    compiled: Vec<Compiled>,
}

impl Default for Policy {
    fn default() -> Self {
        Self::new(NotificationsConfig::default())
    }
}

impl Policy {
    pub fn new(cfg: NotificationsConfig) -> Self {
        let compiled = cfg
            .rules
            .iter()
            .map(|r| Compiled { app: Pattern::new(&r.app), summary: Pattern::new(&r.summary), body: Pattern::new(&r.body), category: Pattern::new(&r.category) })
            .collect();
        Self { cfg, compiled }
    }

    /// The first enabled rule matching `n`.
    pub fn rule_for(&self, n: &Incoming) -> Option<(usize, &NotificationRule)> {
        self.cfg.rules.iter().zip(&self.compiled).enumerate().find_map(|(i, (rule, c))| {
            let app = c.app.matches(n.app) || (!n.desktop_entry.is_empty() && c.app.matches(n.desktop_entry));
            let hit = rule.enabled
                && app
                && c.summary.matches(n.summary)
                && c.body.matches(n.body)
                && c.category.matches(n.category)
                && rule.urgency.is_none_or(|u| Urgency::from(u) == n.urgency);
            hit.then_some((i, rule))
        })
    }

    /// Decides what happens to `n`. `quiet` is do-not-disturb in any form.
    pub fn decide(&self, n: &Incoming, quiet: bool) -> Decision {
        let cfg = &self.cfg;
        let rule = self.rule_for(n);
        let r = rule.map(|(_, r)| r);
        let urgency = r.and_then(|r| r.set_urgency).map(Urgency::from).unwrap_or(n.urgency);
        let block = r.is_some_and(|r| r.block);
        let critical = urgency == Urgency::Critical;
        let dnd_pass = !quiet || r.is_some_and(|r| r.bypass_dnd) || (critical && cfg.dnd_allow_critical);
        let wanted = r.is_none_or(|r| r.popup) && UrgencyLevel::from(urgency) >= cfg.min_urgency;
        let popup = !block && wanted && dnd_pass;
        let history = !block && cfg.history && !n.transient && r.is_none_or(|r| r.history) && (urgency != Urgency::Low || cfg.history_low) && cfg.history_max > 0;
        let sound = popup && !n.suppress_sound && r.is_none_or(|r| r.sound);
        let display_ms = match r.and_then(|r| r.timeout) {
            Some(s) => seconds_to_ms(s),
            None if cfg.respect_app_timeout && n.expire_timeout >= 0 => n.expire_timeout.unsigned_abs(),
            None => seconds_to_ms(cfg.timeout_for(urgency.into())),
        };
        Decision { block, popup, history, sound, urgency, display_ms, rule: rule.map(|(i, _)| i) }
    }
}

fn seconds_to_ms(s: f32) -> u32 {
    (s.max(0.0) * MS_PER_S).round() as u32
}

/// Parses `HH:MM`.
pub fn parse_hhmm(s: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(s.trim(), "%H:%M").ok()
}

/// Whether `now` falls in the daily window `[from, to)`, which may wrap
/// past midnight. An empty window (from == to) is never quiet.
pub fn in_window(from: NaiveTime, to: NaiveTime, now: NaiveTime) -> bool {
    if from <= to { now >= from && now < to } else { now >= from || now < to }
}

/// Quiet hours in effect at `now` (local time).
pub fn quiet_hours_active(cfg: &NotificationsConfig, now: NaiveTime) -> bool {
    cfg.quiet_hours && matches!((parse_hhmm(&cfg.quiet_from), parse_hhmm(&cfg.quiet_to)), (Some(f), Some(t)) if in_window(f, t, now))
}

// ── Markup ───────────────────────────────────────────────────────────────

/// A styled run of a notification body (byte offsets into the plain text).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub underline: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
}

fn decode_entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{a0}'),
        n if n.starts_with("#x") || n.starts_with("#X") => u32::from_str_radix(&n[2..], 16).ok().and_then(char::from_u32),
        n if n.starts_with('#') => n[1..].parse().ok().and_then(char::from_u32),
        _ => None,
    }
}

/// The `href` of an `<a …>` tag body.
fn href(tag: &str) -> Option<String> {
    let i = tag.find("href=")?;
    let rest = &tag[i + "href=".len()..];
    let quote = rest.chars().next()?;
    let url = if quote == '"' || quote == '\'' { rest[1..].split(quote).next()? } else { rest.split_whitespace().next()? };
    Some(url.to_string()).filter(|u| !u.is_empty())
}

/// Parses the spec's body markup into plain text plus styled spans.
/// Unknown tags are dropped; `<br>` becomes a newline; entities decode.
pub fn parse_markup(src: &str) -> (String, Vec<Span>) {
    let mut out = String::with_capacity(src.len());
    let mut spans = Vec::new();
    let (mut bold, mut italic, mut underline) = (0u32, 0u32, 0u32);
    let mut link: Option<String> = None;
    let mut run_start = 0usize;
    let flush = |out: &str, start: &mut usize, spans: &mut Vec<Span>, b: u32, i: u32, u: u32, link: &Option<String>| {
        if out.len() > *start && (b > 0 || i > 0 || u > 0 || link.is_some()) {
            spans.push(Span { start: *start, end: out.len(), bold: b > 0, italic: i > 0, underline: u > 0, link: link.clone() });
        }
        *start = out.len();
    };
    let mut rest = src;
    while let Some(c) = rest.chars().next() {
        match c {
            '<' => {
                let Some(close) = rest.find('>') else {
                    out.push_str(rest);
                    break;
                };
                let tag = rest[1..close].trim();
                let closing = tag.starts_with('/');
                let name = tag.trim_start_matches('/').split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or_default().to_ascii_lowercase();
                flush(&out, &mut run_start, &mut spans, bold, italic, underline, &link);
                let step = |n: &mut u32| if closing { *n = n.saturating_sub(1) } else { *n += 1 };
                match name.as_str() {
                    "b" | "strong" => step(&mut bold),
                    "i" | "em" => step(&mut italic),
                    "u" => step(&mut underline),
                    "a" => link = if closing { None } else { href(tag) },
                    "br" => {
                        out.push('\n');
                        run_start = out.len();
                    }
                    _ => {}
                }
                rest = &rest[close + 1..];
            }
            '&' => {
                let decoded = rest.find(';').filter(|&e| e <= 10).and_then(|e| decode_entity(&rest[1..e]).map(|ch| (ch, e)));
                match decoded {
                    Some((ch, e)) => {
                        out.push(ch);
                        rest = &rest[e + 1..];
                    }
                    None => {
                        out.push('&');
                        rest = &rest[1..];
                    }
                }
            }
            c => {
                out.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    flush(&out, &mut run_start, &mut spans, bold, italic, underline, &link);
    (out, spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn incoming<'a>(app: &'a str, summary: &'a str, urgency: Urgency) -> Incoming<'a> {
        Incoming { app, summary, urgency, expire_timeout: -1, ..Default::default() }
    }

    #[test]
    fn markup_parses_styles_links_and_entities() {
        let (text, spans) = parse_markup("Hi <b>bold <i>both</i></b> &amp; <a href=\"https://x.org\">link</a><br/>next &#65;&#x42; &bogus;");
        assert_eq!(text, "Hi bold both & link\nnext AB &bogus;");
        assert_eq!(&text[spans[0].start..spans[0].end], "bold ");
        assert!(spans[0].bold && !spans[0].italic);
        assert!(spans[1].bold && spans[1].italic && &text[spans[1].start..spans[1].end] == "both");
        let l = spans.iter().find(|s| s.link.is_some()).unwrap();
        assert_eq!((&text[l.start..l.end], l.link.as_deref()), ("link", Some("https://x.org")));
        // A lone `<` without a closing `>` survives.
        assert_eq!(parse_markup("a < b").0, "a < b");
    }

    #[test]
    fn rules_match_first_enabled_and_apply_effects() {
        let mut cfg = NotificationsConfig::default();
        cfg.rules = vec![
            NotificationRule { enabled: false, block: true, ..NotificationRule::for_app("spotify") },
            NotificationRule { popup: false, sound: false, ..NotificationRule::for_app("^spot") },
            NotificationRule { set_urgency: Some(UrgencyLevel::Critical), timeout: Some(2.0), summary: "backup (done|failed)".into(), ..NotificationRule::default() },
            NotificationRule { block: true, ..NotificationRule::for_app("spam[") },
        ];
        let p = Policy::new(cfg);
        let d = p.decide(&incoming("Spotify", "Song", Urgency::Normal), false);
        assert_eq!((d.popup, d.history, d.sound, d.rule), (false, true, false, Some(1)));
        let d = p.decide(&incoming("Borg", "Backup failed", Urgency::Low), false);
        assert_eq!((d.urgency, d.display_ms, d.popup), (Urgency::Critical, 2000, true));
        // An invalid regex falls back to plain text.
        assert!(p.decide(&incoming("SPAM[bot]", "x", Urgency::Normal), false).block);
        // Desktop entries match too.
        let n = Incoming { desktop_entry: "spotify", ..incoming("Music", "x", Urgency::Normal) };
        assert_eq!(p.decide(&n, false).rule, Some(1));
    }

    #[test]
    fn dnd_timeouts_and_history() {
        let mut cfg = NotificationsConfig::default();
        let p = Policy::new(cfg.clone());
        // Quiet: only critical passes.
        assert!(!p.decide(&incoming("a", "s", Urgency::Normal), true).popup);
        assert!(p.decide(&incoming("a", "s", Urgency::Critical), true).popup);
        // Timeouts: urgency defaults, the app's own, critical sticks.
        assert_eq!(p.decide(&incoming("a", "s", Urgency::Low), false).display_ms, 4000);
        assert_eq!(p.decide(&Incoming { expire_timeout: 1500, ..incoming("a", "s", Urgency::Normal) }, false).display_ms, 1500);
        assert_eq!(p.decide(&incoming("a", "s", Urgency::Critical), false).display_ms, 0);
        cfg.respect_app_timeout = false;
        cfg.dnd_allow_critical = false;
        cfg.history_low = false;
        cfg.min_urgency = UrgencyLevel::Normal;
        let p = Policy::new(cfg);
        assert_eq!(p.decide(&Incoming { expire_timeout: 1500, ..incoming("a", "s", Urgency::Normal) }, false).display_ms, 6000);
        assert!(!p.decide(&incoming("a", "s", Urgency::Critical), true).popup);
        let low = p.decide(&incoming("a", "s", Urgency::Low), false);
        assert!(!low.popup && !low.history);
        assert!(!p.decide(&Incoming { transient: true, ..incoming("a", "s", Urgency::Normal) }, false).history);
    }

    #[test]
    fn quiet_hours_wrap_midnight() {
        let t = |s| parse_hhmm(s).unwrap();
        assert!(in_window(t("22:00"), t("07:00"), t("23:30")));
        assert!(in_window(t("22:00"), t("07:00"), t("06:59")));
        assert!(!in_window(t("22:00"), t("07:00"), t("07:00")));
        assert!(in_window(t("09:00"), t("17:00"), t("12:00")));
        assert!(!in_window(t("09:00"), t("09:00"), t("09:00")));
        let mut cfg = NotificationsConfig { quiet_hours: true, ..Default::default() };
        assert!(quiet_hours_active(&cfg, t("23:00")));
        cfg.quiet_from = "bogus".into();
        assert!(!quiet_hours_active(&cfg, t("23:00")));
    }
}
