//! The lines the root helper writes on its standard output, between the
//! package manager's own output. A marker line starts with the ASCII
//! record separator, which package managers never print, followed by
//! `mandara`:
//!
//! ```text
//! ␞mandara begin pacman 2        the plan: backend, number of steps
//! ␞mandara step 1 download       a step starts (1-based)
//! ␞mandara exit 1 0              it exited (code, or `signal`)
//! ␞mandara cancel ok|refused     answer to a cancel request
//! ␞mandara reboot                the system asks for a restart
//! ␞mandara done ok|cancelled|failed:<kind>
//! ```
//!
//! On its standard input the helper accepts one request: `cancel`. It is
//! honoured only while a cancellable step runs; closing the input (the
//! shell went away) never stops an update.

use super::FailureKind;
use super::plan::StepKind;

/// Starts every marker line.
pub const MARK: &str = "\u{1e}mandara ";
/// Version of this protocol, printed by `mandara system-update --protocol`
/// (no root needed) so the shell knows the admin helper understands it.
pub const PROTOCOL: u32 = 1;
/// The cancel request on the helper's standard input.
pub const CANCEL: &str = "cancel";
/// Longest output line kept (longer lines are cut at a char boundary).
pub const MAX_LINE: usize = 1024;

/// One parsed marker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mark {
    Begin { backend: String, steps: u32 },
    Step { index: u32, kind: StepKind },
    Exit { index: u32, code: Option<i32> },
    Cancel { accepted: bool },
    Reboot,
    Done(Outcome),
}

/// How the helper finished.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Cancelled,
    Failed(FailureKind),
}

const FAILURE_IDS: [(FailureKind, &str); 9] = [
    (FailureKind::Locked, "locked"),
    (FailureKind::Network, "network"),
    (FailureKind::Conflict, "conflict"),
    (FailureKind::DiskFull, "disk-full"),
    (FailureKind::Keyring, "keyring"),
    (FailureKind::NotAuthorized, "not-authorized"),
    (FailureKind::NoPrompt, "no-prompt"),
    (FailureKind::Helper, "helper"),
    (FailureKind::Other, "other"),
];

fn failure_id(kind: FailureKind) -> &'static str {
    FAILURE_IDS.iter().find(|(k, _)| *k == kind).map_or("other", |(_, id)| id)
}

impl Mark {
    /// The marker as one line (without the newline).
    pub fn line(&self) -> String {
        match self {
            Mark::Begin { backend, steps } => format!("{MARK}begin {backend} {steps}"),
            Mark::Step { index, kind } => format!("{MARK}step {index} {}", kind.id()),
            Mark::Exit { index, code: Some(c) } => format!("{MARK}exit {index} {c}"),
            Mark::Exit { index, code: None } => format!("{MARK}exit {index} signal"),
            Mark::Cancel { accepted } => format!("{MARK}cancel {}", if *accepted { "ok" } else { "refused" }),
            Mark::Reboot => format!("{MARK}reboot"),
            Mark::Done(Outcome::Ok) => format!("{MARK}done ok"),
            Mark::Done(Outcome::Cancelled) => format!("{MARK}done cancelled"),
            Mark::Done(Outcome::Failed(k)) => format!("{MARK}done failed:{}", failure_id(*k)),
        }
    }

    /// Parses a marker line; `None` for ordinary output (or a malformed
    /// marker, which is then shown as output).
    pub fn parse(line: &str) -> Option<Mark> {
        let rest = line.strip_prefix(MARK)?;
        let mut w = rest.split(' ');
        let mark = match w.next()? {
            "begin" => Mark::Begin { backend: w.next()?.to_string(), steps: w.next()?.parse().ok()? },
            "step" => Mark::Step { index: w.next()?.parse().ok()?, kind: StepKind::from_id(w.next()?)? },
            "exit" => {
                let index = w.next()?.parse().ok()?;
                let code = match w.next()? {
                    "signal" => None,
                    c => Some(c.parse().ok()?),
                };
                Mark::Exit { index, code }
            }
            "cancel" => Mark::Cancel { accepted: w.next()? == "ok" },
            "reboot" => Mark::Reboot,
            "done" => Mark::Done(match w.next()? {
                "ok" => Outcome::Ok,
                "cancelled" => Outcome::Cancelled,
                f => {
                    let id = f.strip_prefix("failed:")?;
                    Outcome::Failed(FAILURE_IDS.iter().find(|(_, i)| *i == id).map_or(FailureKind::Other, |(k, _)| *k))
                }
            }),
            _ => return None,
        };
        w.next().is_none().then_some(mark)
    }
}

/// `line` cut to [`MAX_LINE`] bytes at a char boundary, without control
/// characters other than tab (terminal escapes are dropped).
pub fn clean_line(line: &str) -> std::borrow::Cow<'_, str> {
    let cut = if line.len() > MAX_LINE { &line[..line.floor_char_boundary(MAX_LINE)] } else { line };
    if !cut.contains(|c: char| c.is_control() && c != '\t') {
        return std::borrow::Cow::Borrowed(cut.trim_end());
    }
    let mut out = String::with_capacity(cut.len());
    let mut chars = cut.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // CSI escape: ESC [ … final byte in @..~.
            '\u{1b}' => {
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for d in chars.by_ref() {
                        if ('@'..='~').contains(&d) {
                            break;
                        }
                    }
                }
            }
            '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    std::borrow::Cow::Owned(out.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markers_round_trip() {
        let marks = [
            Mark::Begin { backend: "pacman".into(), steps: 2 },
            Mark::Step { index: 1, kind: StepKind::Download },
            Mark::Exit { index: 1, code: Some(0) },
            Mark::Exit { index: 2, code: Some(-1) },
            Mark::Exit { index: 2, code: None },
            Mark::Cancel { accepted: true },
            Mark::Cancel { accepted: false },
            Mark::Reboot,
            Mark::Done(Outcome::Ok),
            Mark::Done(Outcome::Cancelled),
            Mark::Done(Outcome::Failed(FailureKind::Locked)),
            Mark::Done(Outcome::Failed(FailureKind::DiskFull)),
        ];
        for m in marks {
            assert_eq!(Mark::parse(&m.line()), Some(m.clone()), "{}", m.line());
        }
    }

    #[test]
    fn ordinary_and_malformed_lines_are_output() {
        for line in ["mandara step 1 download", ":: Synchronizing package databases...", "\u{1e}mandara step x download", "\u{1e}mandara step 1 rm-rf", "\u{1e}mandara done ok extra", "\u{1e}mandara nope", ""] {
            assert_eq!(Mark::parse(line), None, "{line:?}");
        }
    }

    #[test]
    fn lines_are_cleaned_and_bounded() {
        assert_eq!(clean_line("\u{1b}[1;32m:: done\u{1b}[0m  "), ":: done");
        assert_eq!(clean_line("a\tb\u{7}c"), "a\tbc");
        let long = "é".repeat(MAX_LINE);
        let cut = clean_line(&long);
        assert!(cut.len() <= MAX_LINE && cut.chars().all(|c| c == 'é'));
        assert!(matches!(clean_line("plain"), std::borrow::Cow::Borrowed("plain")));
    }
}
