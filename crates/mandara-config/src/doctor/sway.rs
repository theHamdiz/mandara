//! Sway: `sway --validate --verbose --config FILE` (verified against sway
//! 1.12).
//!
//! Sway sets up its backend before it reads the config, even to validate,
//! so the check runs with a headless, software one ([`SWAY_CHECK_ENV`]) in
//! the doctor's private runtime dir: no display, seat or GPU is touched.
//! Problems are log lines:
//!
//! ```text
//! 00:00:00.004 [ERROR] [sway/config.c:814] Error on line 2 'frobnicate on': Unknown/invalid command 'frobnicate' (/home/u/.config/sway/config)
//! 00:00:00.004 [ERROR] [sway/config.c:440] Error(s) loading config!
//! 00:00:00.003 [INFO] [sway/commands/bind.c:286] Overwriting binding 'Mod4+q' for device '*' to `exec foot` from `kill`
//! ```
//!
//! The line number counts in the file named at the end (an included file's
//! own lines). Sway exits 0 when the error is in an included file, so any
//! line at level `[ERROR]` means the config was rejected (only the level's
//! place counts: `--verbose` echoes config lines). `--verbose` is what
//! reports a bind defined twice; the rest of the verbose output (the
//! environment, kernel and os-release) is left out of the report.

use std::ffi::OsString;
use std::path::Path;

use super::{CompositorCheck, DiagKind, Diagnostic, Output, ParseContext, SWAY_CHECK_ENV, Severity};
use crate::compositor::Target;

pub struct Sway;

impl CompositorCheck for Sway {
    fn target(&self) -> Target {
        Target::Sway
    }

    fn program(&self) -> &'static str {
        "sway"
    }

    fn args(&self, config: &Path) -> Vec<OsString> {
        vec!["--validate".into(), "--verbose".into(), "--config".into(), config.as_os_str().to_owned()]
    }

    fn env(&self) -> &'static [(&'static str, &'static str)] {
        SWAY_CHECK_ENV
    }

    fn parse(&self, out: &Output, ctx: &ParseContext) -> Vec<Diagnostic> {
        parse(&out.text(), ctx)
    }

    fn rejected(&self, out: &Output) -> bool {
        !out.success || has_error(&out.text())
    }

    fn clean_raw(&self, text: &str) -> String {
        text.lines().filter(|l| level(l) == Some(ERROR) || l.contains(OVERWRITING)).collect::<Vec<_>>().join("\n")
    }

    /// The command a line runs (`gaps inner 8` → `gaps`); for `input` and
    /// `seat` lines the setting (`input type:touchpad tap enabled` → `tap`),
    /// the word sway names when it rejects one.
    fn line_key(&self, line: &str) -> Option<String> {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            [first, ..] if first.starts_with('#') => None,
            ["input" | "seat", _, setting, ..] => Some((*setting).to_string()),
            [first, ..] => Some(first.trim_end_matches('{').to_string()).filter(|w| !w.is_empty() && *w != "}"),
            [] => None,
        }
    }
}

/// The log level of an error line.
const ERROR: &str = "ERROR";
const OVERWRITING: &str = "Overwriting binding '";

/// A log line's level and the text after it: sway writes
/// `HH:MM:SS.mmm [LEVEL] [source.c:123] message`, the level the first
/// bracketed word after the time stamp. `None` for any other line, such as
/// a config line echoed into an `[INFO]` message (which may itself say
/// `[ERROR]`).
fn split_level(line: &str) -> Option<(&str, &str)> {
    let line = line.trim_start();
    let stamp = line.split_once(' ').filter(|(t, _)| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit() || b == b':' || b == b'.'));
    let rest = stamp.map_or(line, |(_, rest)| rest.trim_start());
    let (level, rest) = rest.strip_prefix('[')?.split_once(']')?;
    (!level.is_empty() && level.bytes().all(|b| b.is_ascii_uppercase())).then_some((level, rest))
}

/// A log line's level ([`split_level`]).
fn level(line: &str) -> Option<&str> {
    split_level(line).map(|(level, _)| level)
}

/// Whether sway's output has an error line (at the level's place only).
pub(crate) fn has_error(text: &str) -> bool {
    text.lines().any(|l| level(l) == Some(ERROR))
}

/// The message of an `[ERROR]` line, after its source location.
fn error_message(line: &str) -> Option<&str> {
    let (level, rest) = split_level(line)?;
    if level != ERROR {
        return None;
    }
    let rest = rest.trim_start();
    // The source location, when sway prints one.
    Some(match rest.strip_prefix('[').and_then(|r| r.split_once("] ")) {
        Some((_, msg)) => msg,
        None => rest,
    })
}

pub(super) fn parse(text: &str, ctx: &ParseContext) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for line in text.lines() {
        if let Some(msg) = error_message(line) {
            if msg.starts_with("Error(s) loading config") {
                continue;
            }
            out.push(line_error(msg, ctx).unwrap_or_else(|| Diagnostic::new(Severity::Error, msg.trim())));
        } else if let Some(rest) = line.split_once(OVERWRITING).map(|(_, r)| r) {
            let combo = rest.split('\'').next().unwrap_or_default();
            out.push(Diagnostic::new(Severity::Warning, format!("{OVERWRITING}{rest}")).kind(DiagKind::BindConflict).key(combo));
        }
    }
    out
}

/// `Error on line N '<line>': <message> (<file>)`.
fn line_error(msg: &str, ctx: &ParseContext) -> Option<Diagnostic> {
    let rest = msg.strip_prefix("Error on line ")?;
    let (number, rest) = rest.split_once(' ')?;
    let line: usize = number.parse().ok()?;
    let body = rest.strip_prefix('\'')?;
    let (_, after) = body.split_once("': ")?;
    let (message, file) = match after.trim_end().strip_suffix(')').and_then(|r| r.rsplit_once(" (")) {
        Some((m, f)) => (m.trim(), Some(f)),
        None => (after.trim(), None),
    };
    let mut d = Diagnostic::new(Severity::Error, message);
    if let Some(file) = file {
        d = d.at(ctx.resolve(file), line, None);
    }
    Some(classify(d))
}

/// Kind and key from sway's wording.
fn classify(d: Diagnostic) -> Diagnostic {
    let m = d.message.clone();
    let quoted = |prefix: &str| m.strip_prefix(prefix).and_then(|r| r.split('\'').next()).map(str::to_string);
    if let Some(key) = quoted("Unknown/invalid command '") {
        return d.kind(DiagKind::UnknownOption).key(key);
    }
    if let Some(sub) = m.strip_prefix("Invalid output subcommand: ") {
        return d.kind(DiagKind::UnknownOption).key(sub.trim_end_matches('.').trim());
    }
    if m.starts_with("Token '") || m.contains("expected at least") || m.contains("Unmatched") {
        return d.kind(DiagKind::SyntaxError);
    }
    if m.contains("deprecated") {
        return d.kind(DiagKind::Deprecated);
    }
    if m.starts_with("Expected ") || m.starts_with("Invalid ") || m.starts_with("Unknown key") || m.contains("Regex compilation") || m.contains("invalid") || m.contains("expected") {
        return d.kind(DiagKind::InvalidValue);
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_level_position_counts() {
        let error = "00:00:00.004 [ERROR] [sway/config.c:814] Error on line 2 'frobnicate on': Unknown/invalid command 'frobnicate' (/c)";
        assert!(has_error(error));
        assert_eq!(error_message(error), Some("Error on line 2 'frobnicate on': Unknown/invalid command 'frobnicate' (/c)"));
        // A valid line that mentions `[ERROR]`, echoed by `--verbose`.
        let echoed = "00:00:00.003 [INFO] [sway/config.c:870] Read line 3: exec notify-send \"[ERROR] backup\"";
        assert!(!has_error(echoed));
        assert_eq!(error_message(echoed), None);
        assert_eq!(level(echoed), Some("INFO"));
        // No time stamp, or text that only looks like a level.
        assert!(has_error("[ERROR] [sway/main.c:1] no backend"));
        assert!(!has_error("exec foo [ERROR]"));
        assert!(!has_error("00:00:00.001 Loading [ERROR] x"));
        let report = format!("{echoed}\n{error}\n00:00:00.004 [INFO] [sway/commands/bind.c:286] Overwriting binding 'Mod4+q' for device '*' to `exec foot` from `kill`");
        assert_eq!(Sway.clean_raw(&report).lines().count(), 2, "the echo is left out");
    }
}
