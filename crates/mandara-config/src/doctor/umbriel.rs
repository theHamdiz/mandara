//! Umbriel: `umbriel config validate -c FILE --json` (verified against
//! Umbriel 0.1.0).
//!
//! The JSON report on stdout is structured already:
//!
//! ```json
//! {"diagnostics":[{"column":18,"file":"/home/u/.config/umbriel/config.toml",
//!   "line":2,"message":"unknown key general.blur_optimized","severity":"warning"}],
//!  "path":"…","valid":false}
//! ```
//!
//! Without JSON (an older build) the text form is read instead:
//! `warning: FILE:LINE:COL: message` / `error: …`.

use std::ffi::OsString;
use std::path::Path;

use serde::Deserialize;

use super::{CompositorCheck, DiagKind, Diagnostic, Output, ParseContext, Severity};
use crate::compositor::{Target, toml_header, toml_line_key};

pub struct Umbriel;

impl CompositorCheck for Umbriel {
    fn target(&self) -> Target {
        Target::Umbriel
    }

    fn program(&self) -> &'static str {
        "umbriel"
    }

    fn args(&self, config: &Path) -> Vec<OsString> {
        vec!["config".into(), "validate".into(), "-c".into(), config.as_os_str().to_owned(), "--json".into()]
    }

    fn line_key(&self, line: &str) -> Option<String> {
        toml_line_key(line).or_else(|| toml_header(line).map(str::to_string))
    }

    fn parse(&self, out: &Output, ctx: &ParseContext) -> Vec<Diagnostic> {
        parse(&out.stdout, &out.stderr, ctx)
    }
}

#[derive(Deserialize)]
struct Report {
    diagnostics: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    file: Option<String>,
    line: Option<usize>,
    column: Option<usize>,
    message: String,
    severity: String,
}

pub(super) fn parse(stdout: &str, stderr: &str, ctx: &ParseContext) -> Vec<Diagnostic> {
    // The JSON document is the line starting with `{` (logs go to stderr).
    let json = stdout.lines().find(|l| l.trim_start().starts_with('{')).and_then(|l| serde_json::from_str::<Report>(l).ok());
    if let Some(report) = json {
        return report
            .diagnostics
            .into_iter()
            .map(|e| {
                let mut d = Diagnostic::new(severity(&e.severity), e.message);
                if let (Some(file), Some(line)) = (e.file, e.line) {
                    d = d.at(ctx.resolve(&file), line, e.column);
                }
                classify(d)
            })
            .collect();
    }
    // Text form: `warning: /path:2:18: unknown key …`.
    let text = super::strip_ansi(&format!("{stdout}\n{stderr}"));
    text.lines()
        .filter_map(|l| {
            let (sev, rest) = l.split_once(": ")?;
            if !matches!(sev, "warning" | "error") {
                return None;
            }
            let mut parts = rest.splitn(4, ':');
            let (file, line, col, message) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
            let d = Diagnostic::new(severity(sev), message.trim()).at(ctx.resolve(file), line.trim().parse().ok()?, col.trim().parse().ok());
            Some(classify(d))
        })
        .collect()
}

fn severity(s: &str) -> Severity {
    match s {
        "error" => Severity::Error,
        "warning" => Severity::Warning,
        _ => Severity::Info,
    }
}

/// Kind and key from Umbriel's wording.
fn classify(d: Diagnostic) -> Diagnostic {
    let m = d.message.clone();
    if let Some(key) = m.strip_prefix("unknown key ") {
        return d.kind(DiagKind::UnknownOption).key(key.trim());
    }
    if m.starts_with("duplicate keybind") {
        return d.kind(DiagKind::BindConflict);
    }
    if m.starts_with("include not found") || m.starts_with("config file not found") {
        let key = m.split_once(": ").map(|(_, r)| r.split(" (from").next().unwrap_or(r).trim().to_string());
        let d = d.kind(DiagKind::MissingInclude);
        return match key {
            Some(k) => d.key(k),
            None => d,
        };
    }
    if m.starts_with("Error while parsing") || m.contains("cannot redefine") || m.contains("syntax") {
        return d.kind(DiagKind::SyntaxError);
    }
    if m.contains("deprecated") || m.contains("renamed") {
        return d.kind(DiagKind::Deprecated);
    }
    if let Some(rest) = m.strip_prefix("ignoring ") {
        // `ignoring layout.gap (expected integer)`, `ignoring keybind 'X' (…)`.
        let key = rest.split([' ', '(']).next().unwrap_or_default().trim_matches('\'').to_string();
        let d = d.kind(DiagKind::InvalidValue);
        return if key == "keybind" { d } else { d.key(key) };
    }
    if m.contains("invalid") || m.contains("expected") || m.contains("must") {
        return d.kind(DiagKind::InvalidValue);
    }
    d
}
