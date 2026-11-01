//! Niri: `niri validate -c FILE` (verified against Niri 26.04's own config
//! parser, `niri-config`).
//!
//! Niri reports through `miette`. With `NO_GRAPHICS=1` (set by the doctor)
//! it narrates each problem with exact positions:
//!
//! ```text
//! Error: unexpected node `blur_optimized`
//!     Diagnostic severity: error
//!
//! Begin snippet for config.kdl starting at line 2, column 1
//!
//! snippet line 2:     gaps 16
//! snippet line 3:     blur_optimized true
//!     label starting at line 3, column 5: unexpected node
//! ```
//!
//! File names are relative to the main config's folder (absolute when an
//! include lies outside it). The graphical form (`╭─[config.kdl:2:1]`, whose
//! position is the snippet's first line, not the problem's) is read too, in
//! case a future Niri ignores `NO_GRAPHICS`. Wrapper messages ("error
//! loading config", "error parsing KDL") are skipped, and an include whose
//! file failed to parse is dropped when that file's own problems are listed.

use std::ffi::OsString;
use std::path::Path;

use super::{CompositorCheck, DiagKind, Diagnostic, Location, Output, ParseContext, Severity};
use crate::compositor::Target;

pub struct Niri;

impl CompositorCheck for Niri {
    fn target(&self) -> Target {
        Target::Niri
    }

    fn program(&self) -> &'static str {
        "niri"
    }

    fn args(&self, config: &Path) -> Vec<OsString> {
        vec!["validate".into(), "-c".into(), config.as_os_str().to_owned()]
    }

    fn line_key(&self, line: &str) -> Option<String> {
        let first = line.split_whitespace().next()?;
        let first = first.trim_start_matches("/-").trim_end_matches(['{', ';']);
        (!first.is_empty() && !first.starts_with(['"', '}', '/'])).then(|| first.to_string())
    }

    fn parse(&self, out: &Output, ctx: &ParseContext) -> Vec<Diagnostic> {
        parse(&out.text(), ctx)
    }
}

/// Messages that only wrap the real ones.
const WRAPPERS: &[&str] = &["error loading config", "error parsing", "error parsing KDL"];
/// The include line whose file has its own problems.
const INCLUDE_WRAPPER: &str = "failed to parse included config";

pub(super) fn parse(text: &str, ctx: &ParseContext) -> Vec<Diagnostic> {
    let mut out = if text.contains("Begin snippet for ") || text.contains("Diagnostic severity:") { narrated(text, ctx) } else { graphical(text, ctx) };
    out.extend(tracing_warnings(text));
    // "failed to parse included config" only points at the include line.
    if out.iter().any(|d| d.message != INCLUDE_WRAPPER) {
        out.retain(|d| d.message != INCLUDE_WRAPPER);
    }
    merge_duplicate_binds(out)
}

/// `NO_GRAPHICS` reports.
fn narrated(text: &str, ctx: &ParseContext) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> = Vec::new();
    let mut in_message = false;
    let mut snippet_file: Option<std::path::PathBuf> = None;
    let mut located = false;
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(message) = raw.strip_prefix("Error:").or_else(|| raw.strip_prefix("Warning:")).or_else(|| raw.strip_prefix("Advice:")) {
            let severity = if raw.starts_with("Error") { Severity::Error } else if raw.starts_with("Warning") { Severity::Warning } else { Severity::Info };
            out.push(Diagnostic::new(severity, message.trim()));
            in_message = true;
            snippet_file = None;
            located = false;
            continue;
        }
        let Some(d) = out.last_mut() else { continue };
        if let Some(sev) = line.strip_prefix("Diagnostic severity:") {
            d.severity = match sev.trim() {
                "warning" => Severity::Warning,
                "advice" => Severity::Info,
                _ => Severity::Error,
            };
            in_message = false;
        } else if line.starts_with("Caused by:") {
            in_message = false;
        } else if let Some(rest) = line.strip_prefix("Begin snippet for ") {
            let file = rest.rsplit_once(" starting at line").map_or(rest, |(f, _)| f);
            snippet_file = Some(ctx.resolve(file));
        } else if let Some(rest) = line.strip_prefix("label ") {
            if located || rest.starts_with("ending") {
                continue;
            }
            // `at line 3, columns 5 to 9: …`, `starting at line 3, column 5: …`.
            let after = rest.split_once("line ").map(|(_, r)| r).unwrap_or_default();
            let n: String = after.chars().take_while(char::is_ascii_digit).collect();
            let column = after.split_once("column").and_then(|(_, r)| r.trim_start_matches('s').trim().chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok());
            if let (Ok(n), Some(file)) = (n.parse::<usize>(), snippet_file.clone()) {
                d.file = Some(file);
                d.line = Some(n);
                d.column = column;
                located = true;
            }
        } else if in_message && !line.is_empty() && !line.starts_with("snippet line") {
            // A message continued on the next line.
            d.message.push(' ');
            d.message.push_str(line);
        }
    }
    out.retain(|d| !WRAPPERS.contains(&d.message.as_str()));
    out.into_iter().map(classify).collect()
}

/// The graphical report: `× message`, `╭─[file:line:col]`, numbered source
/// lines and underlines (`·` rows) under the problem's line.
fn graphical(text: &str, ctx: &ParseContext) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> = Vec::new();
    let mut file: Option<std::path::PathBuf> = None;
    let mut last_source: Option<(usize, usize)> = None;
    let mut located = false;
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(message) = line.strip_prefix("Error:").map(str::trim).unwrap_or(line).strip_prefix('×') {
            out.push(Diagnostic::new(Severity::Error, message.trim()));
            file = None;
            last_source = None;
            located = false;
            continue;
        }
        let Some(d) = out.last_mut() else { continue };
        if let Some(rest) = line.strip_prefix("╭─[").and_then(|r| r.strip_suffix(']')) {
            let mut parts = rest.rsplitn(3, ':');
            let (_col, _line, name) = (parts.next(), parts.next(), parts.next());
            file = name.map(|n| ctx.resolve(n));
        } else if let Some((n, _)) = line.split_once('│').filter(|(n, _)| !n.trim().is_empty() && n.trim().chars().all(|c| c.is_ascii_digit())) {
            // ` 3 │     blur_optimized true`: the gutter width is where `│` is.
            let gutter = raw.find('│').unwrap_or_default();
            last_source = n.trim().parse().ok().map(|n| (n, gutter));
        } else if line.starts_with('·') && !located && line.contains(['┬', '─', '^']) {
            if let (Some((n, gutter)), Some(f)) = (last_source, file.clone()) {
                let marker = raw.char_indices().find(|(i, c)| *i > gutter && ['─', '┬', '^'].contains(c)).map(|(i, _)| raw[gutter..i].chars().count());
                d.file = Some(f);
                d.line = Some(n);
                // Columns count from after `│ `.
                d.column = marker.map(|m| m.saturating_sub(1).max(1));
                located = true;
            }
        } else if line.starts_with("│") && d.file.is_none() {
            let more = line.trim_start_matches('│').trim();
            if !more.is_empty() {
                d.message.push(' ');
                d.message.push_str(more);
            }
        }
    }
    out.retain(|d| !WRAPPERS.contains(&d.message.as_str()));
    out.into_iter().map(classify).collect()
}

/// Tracing warnings Niri logs while loading: `… WARN niri_config: optional
/// include not found: "/path"`.
fn tracing_warnings(text: &str) -> Vec<Diagnostic> {
    text.lines()
        .filter(|l| l.contains(" WARN ") && l.contains("niri"))
        .filter_map(|l| {
            let message = l.split_once(" WARN ")?.1.split_once(": ")?.1.trim();
            Some(classify(Diagnostic::new(Severity::Warning, message)))
        })
        .collect()
}

fn quoted(s: &str) -> Option<String> {
    let start = s.find('"')? + 1;
    let len = s[start..].find('"')?;
    Some(s[start..start + len].to_string())
}

/// Kind and key from Niri's wording.
fn classify(d: Diagnostic) -> Diagnostic {
    let m = d.message.to_ascii_lowercase();
    let backticked = d.message.split('`').nth(1).map(str::to_string);
    if m.starts_with("unexpected node") || m.starts_with("unexpected property") || m.starts_with("unexpected argument") || m.starts_with("unknown") {
        let kind = DiagKind::UnknownOption;
        return match backticked {
            Some(k) => d.kind(kind).key(k),
            None => d.kind(kind),
        };
    }
    if m.contains("keybind") && (m.contains("duplicate") || m.contains("first defined")) {
        return d.kind(DiagKind::BindConflict);
    }
    if m.contains("include") && (m.contains("failed to read") || m.contains("not found") || m.contains("no such file")) {
        let key = quoted(&d.message);
        let d = d.kind(DiagKind::MissingInclude);
        return match key {
            Some(k) => d.key(k),
            None => d,
        };
    }
    if m.contains("recursive include") || m.contains("unclosed") || m.contains("unexpected token") || m.contains("unexpected end") || m.starts_with("expected") || m.contains("invalid escape") || m.contains("unterminated") {
        return d.kind(DiagKind::SyntaxError);
    }
    if m.contains("deprecated") {
        return match backticked {
            Some(k) => d.kind(DiagKind::Deprecated).key(k),
            None => d.kind(DiagKind::Deprecated),
        };
    }
    if m.contains("unsupported value") || m.contains("invalid") || m.contains("expected") || m.contains("must be") || m.contains("out of range") || m.contains("missing") || m.contains("error parsing") {
        return d.kind(DiagKind::InvalidValue);
    }
    d
}

/// Niri reports a duplicate bind twice: "keybind first defined here" and
/// "duplicate keybind later defined here". One diagnostic at the later bind,
/// with the first as a related place.
fn merge_duplicate_binds(list: Vec<Diagnostic>) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> = Vec::new();
    let mut first: Option<Location> = None;
    for d in list {
        if d.kind == DiagKind::BindConflict && d.message.contains("first defined") {
            first = d.file.clone().zip(d.line).map(|(file, line)| Location { file, line, column: d.column });
            continue;
        }
        let mut d = d;
        if d.kind == DiagKind::BindConflict
            && let Some(loc) = first.take()
        {
            d.related.push(loc);
        }
        out.push(d);
    }
    out
}
