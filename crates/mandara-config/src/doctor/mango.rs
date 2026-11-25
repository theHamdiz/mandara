//! MangoWM: `mango -p -c FILE` (verified against mango 0.18.4).
//!
//! It prints each problem as a coloured block on stderr:
//!
//! ```text
//! [ERROR]: Unknown keyword: blur_optimizedx
//! ╰─[Index] /home/u/.config/mango/config.conf:2
//!    ╰─blur_optimizedx=1
//! ```
//!
//! Some messages carry no place (`[ERROR]: Failed to open config file:
//! ./x.conf`, `[WARN]: device rule type touchpad is deprecated, use
//! trackpad instead`), and a key-binding conflict lists every place:
//!
//! ```text
//! [WARN]: Key binding conflict in keymode default:
//!   File "/home/u/.config/mango/bind.conf", line 1
//!   File "/home/u/.config/mango/bind.conf", line 3
//! ```
//!
//! Its exit status doesn't follow the result (0 with errors in included
//! files), so only the parsed messages count.

use std::ffi::OsString;
use std::path::Path;

use super::{CompositorCheck, DiagKind, Diagnostic, Location, Output, ParseContext, Severity};
use crate::compositor::Target;

pub struct Mango;

impl CompositorCheck for Mango {
    fn target(&self) -> Target {
        Target::Mango
    }

    fn program(&self) -> &'static str {
        "mango"
    }

    fn args(&self, config: &Path) -> Vec<OsString> {
        vec!["-p".into(), "-c".into(), config.as_os_str().to_owned()]
    }

    fn version_flag(&self) -> &'static str {
        "-v"
    }

    fn rejected(&self, _: &Output) -> bool {
        false
    }

    fn line_key(&self, line: &str) -> Option<String> {
        let (key, _) = line.split_once('=')?;
        let key = key.trim();
        (!key.is_empty() && !key.contains(char::is_whitespace)).then(|| key.to_string())
    }

    fn parse(&self, out: &Output, ctx: &ParseContext) -> Vec<Diagnostic> {
        parse(&out.text(), ctx)
    }
}

/// Messages that stand alone: never folded into the next block.
fn standalone(message: &str) -> bool {
    message.starts_with("Failed to open config file") || message.contains("deprecated") || message.starts_with("Key binding conflict")
}

pub(super) fn parse(text: &str, ctx: &ParseContext) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> = Vec::new();
    // A place-less detail ("Invalid number in array: a") right before the
    // block it explains.
    let mut pending: Option<String> = None;
    let mut expect_text = false;
    for raw in text.lines() {
        let line = raw.trim();
        let header = [("[ERROR]:", Severity::Error), ("[WARN]:", Severity::Warning)].into_iter().find_map(|(tag, sev)| line.strip_prefix(tag).map(|m| (sev, m.trim())));
        if let Some((severity, message)) = header {
            if let Some(prev) = out.last()
                && prev.file.is_none()
                && prev.related.is_empty()
                && !standalone(&prev.message)
            {
                pending = out.pop().map(|d| d.message);
            }
            let mut d = classify(Diagnostic::new(severity, message));
            if let Some(detail) = pending.take() {
                d.message = format!("{} ({detail})", d.message);
            }
            out.push(d);
            expect_text = false;
            continue;
        }
        let Some(d) = out.last_mut() else { continue };
        if let Some(rest) = line.find("[Index]").map(|at| line[at + "[Index]".len()..].trim()) {
            if let Some((file, n)) = rest.rsplit_once(':')
                && let Ok(n) = n.trim().parse::<usize>()
            {
                d.file = Some(ctx.resolve(file));
                d.line = Some(n);
                expect_text = true;
            }
        } else if let Some(rest) = line.strip_prefix("File \"") {
            // `File "path", line N` — a conflict's places, last one primary.
            if let Some((file, tail)) = rest.split_once('"')
                && let Some(n) = tail.trim_start_matches(',').trim().strip_prefix("line").and_then(|n| n.trim().parse::<usize>().ok())
            {
                let place = Location { file: ctx.resolve(file), line: n, column: None };
                if let (Some(file), Some(line)) = (d.file.take(), d.line.take()) {
                    d.related.push(Location { file, line, column: None });
                }
                d.file = Some(place.file);
                d.line = Some(place.line);
            }
        } else if expect_text && let Some(text) = line.strip_prefix("╰─") {
            d.text = Some(text.trim().to_string()).filter(|t| !t.is_empty());
            expect_text = false;
        } else if !line.is_empty() && d.file.is_none() && d.message.ends_with(':') {
            // A message continued on the next line.
            d.message.push(' ');
            d.message.push_str(line);
        }
    }
    out
}

/// Kind and key from MangoWM's message wording.
fn classify(d: Diagnostic) -> Diagnostic {
    let m = d.message.clone();
    let after = |prefix: &str| m.strip_prefix(prefix).map(|r| r.trim().to_string());
    if let Some(key) = after("Unknown keyword:") {
        return d.kind(DiagKind::UnknownOption).key(key);
    }
    if let Some(func) = after("Unknown dispatch in bind:") {
        return d.kind(DiagKind::InvalidValue).key(func);
    }
    if let Some(file) = after("Failed to open config file:") {
        return d.kind(DiagKind::MissingInclude).key(file);
    }
    if m.starts_with("Key binding conflict") {
        return d.kind(DiagKind::BindConflict);
    }
    if m.contains("deprecated") {
        // `device rule type touchpad is deprecated, use trackpad instead`.
        let key = m.split(" is deprecated").next().and_then(|s| s.split_whitespace().last()).unwrap_or_default().to_string();
        return d.kind(DiagKind::Deprecated).key(key);
    }
    if m.starts_with("Invalid line format") || m.starts_with("Invalid bind format") || m.contains("syntax") {
        return d.kind(DiagKind::SyntaxError);
    }
    if m.starts_with("Unknown") && m.contains("option") {
        // `Unknown window rule option: x`.
        let key = m.rsplit(':').next().unwrap_or_default().trim().to_string();
        return d.kind(DiagKind::UnknownOption).key(key);
    }
    if m.starts_with("Invalid") || m.starts_with("Failed to parse") || m.contains("expected") || m.contains("must be") {
        return d.kind(DiagKind::InvalidValue);
    }
    d
}
