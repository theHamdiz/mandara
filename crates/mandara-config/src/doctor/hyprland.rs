//! Hyprland: `Hyprland --verify-config -c FILE`.
//!
//! **Not verified against a real binary** (Hyprland isn't installed where
//! this was written): the format comes from Hyprland's source. In verify
//! mode `CCompositor::initServer` loads the config and prints
//!
//! ```text
//! ======== Config parsing result:
//!
//! <ConfigManager::verify()>
//! ```
//!
//! after any log lines. `verify()` is `config ok`, or the errors one per
//! line:
//!
//! - Lua configs (0.55+ with `hyprland.lua`, the only kind from 0.57):
//!   `"<chunk>:<line>: <message>"` (`getSourceInfo`; Lua shortens long chunk
//!   names to `...tail`), a load error the same way, runtime errors with a
//!   `stack traceback:` and tab-indented frames after them, and some with a
//!   `context: ` prefix (`config reload: …`, `require("x"): …`);
//! - hyprlang configs (up to 0.56): `Config error in file <path> at line
//!   <n>: <message>`, nested for `source`d files (`… at line 9: Config error
//!   in file <child> at line 2: …`), plus `Config error at line <n>: …` and
//!   `Config error in file <path>: Unclosed category at EOF`.
//!
//! Anything else becomes one diagnostic with the raw text, so an unexpected
//! format is still shown rather than lost.

use std::ffi::OsString;
use std::path::Path;

use super::{CompositorCheck, DiagKind, Diagnostic, Output, ParseContext, Severity};
use crate::compositor::Target;

pub struct Hyprland;

/// The line `--verify-config` prints before its result.
const RESULT_MARKER: &str = "======== Config parsing result:";
/// What `verify()` says when there is nothing wrong.
const CONFIG_OK: &str = "config ok";

impl CompositorCheck for Hyprland {
    fn target(&self) -> Target {
        Target::Hyprland
    }

    fn program(&self) -> &'static str {
        "Hyprland"
    }

    fn args(&self, config: &Path) -> Vec<OsString> {
        vec!["--verify-config".into(), "-c".into(), config.as_os_str().to_owned()]
    }

    fn verified(&self) -> bool {
        false
    }

    fn line_key(&self, line: &str) -> Option<String> {
        let line = line.trim();
        // hyprlang `key = value`; Lua `key = value,` inside `hl.config{…}`.
        let (key, _) = line.split_once('=')?;
        let key = key.trim().trim_start_matches("local ");
        (!key.is_empty() && key.chars().all(|c| c.is_alphanumeric() || "_.:-".contains(c))).then(|| key.to_string())
    }

    fn parse(&self, out: &Output, ctx: &ParseContext) -> Vec<Diagnostic> {
        parse(&out.text(), ctx)
    }
}

pub(super) fn parse(text: &str, ctx: &ParseContext) -> Vec<Diagnostic> {
    let result = text.rsplit_once(RESULT_MARKER).map_or(text, |(_, r)| r).trim();
    if result.is_empty() || result == CONFIG_OK {
        return Vec::new();
    }
    let mut out: Vec<Diagnostic> = Vec::new();
    let mut in_traceback = false;
    for line in result.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if line.trim_start().starts_with("stack traceback:") {
            in_traceback = true;
            continue;
        }
        if in_traceback && line.starts_with(['\t', ' ']) {
            continue;
        }
        in_traceback = false;
        let d = hyprlang(line, ctx).or_else(|| lua(line, ctx)).unwrap_or_else(|| Diagnostic::new(Severity::Error, line.trim()));
        out.push(classify(d));
    }
    out
}

/// `Config error in file X at line N: msg` (innermost of nested ones).
fn hyprlang(line: &str, ctx: &ParseContext) -> Option<Diagnostic> {
    const IN_FILE: &str = "Config error in file ";
    const AT_LINE: &str = "Config error at line ";
    if let Some(at) = line.rfind(IN_FILE) {
        let rest = &line[at + IN_FILE.len()..];
        if let Some((file, tail)) = rest.split_once(" at line ")
            && let Some((n, message)) = tail.split_once(": ")
            && let Ok(n) = n.trim().parse::<usize>()
        {
            return Some(Diagnostic::new(Severity::Error, message.trim()).at(ctx.resolve(file), n, None));
        }
        // `Config error in file X: Unclosed category at EOF`.
        let (file, message) = rest.split_once(": ")?;
        let mut d = Diagnostic::new(Severity::Error, message.trim());
        d.file = Some(ctx.resolve(file));
        return Some(d);
    }
    if let Some(rest) = line.find(AT_LINE).map(|at| &line[at + AT_LINE.len()..]) {
        let (n, message) = rest.split_once(": ")?;
        return Some(Diagnostic::new(Severity::Error, message.trim()).at(ctx.root.clone(), n.trim().parse().ok()?, None));
    }
    line.trim().strip_prefix("Config error: ").map(|m| Diagnostic::new(Severity::Error, m.trim()))
}

/// `[context: ]/path/file.lua:12: message`.
fn lua(line: &str, ctx: &ParseContext) -> Option<Diagnostic> {
    let at = line.find(".lua:")?;
    let start = line[..at].rfind(|c: char| c.is_whitespace() || c == '"').map_or(0, |i| i + 1);
    let file = &line[start..at + ".lua".len()];
    let rest = &line[at + ".lua:".len()..];
    let (n, message) = rest.split_once(':')?;
    let n = n.trim().parse::<usize>().ok()?;
    let mut message = message.trim().to_string();
    // A `require("x"): ` prefix names the module that failed.
    if start > 0 {
        let prefix = line[..start].trim().trim_end_matches(':').trim();
        if prefix.starts_with("require") {
            message = format!("{prefix}: {message}");
        }
    }
    Some(Diagnostic::new(Severity::Error, message).at(ctx.resolve(file), n, None))
}

/// Kind and key from Hyprland's wording.
fn classify(d: Diagnostic) -> Diagnostic {
    let m = d.message.clone();
    let lower = m.to_ascii_lowercase();
    // hyprlang `config option <general:foo> does not exist.`
    if let Some(key) = m.strip_prefix("config option <").and_then(|r| r.split_once('>')).map(|(k, _)| k.to_string()) {
        return d.kind(DiagKind::UnknownOption).key(key);
    }
    // Lua `unknown config key 'general.foo'`, `hl.window_rule: unknown field 'x'`.
    if lower.contains("unknown config key") || lower.contains("unknown field") || lower.contains("unknown match property") {
        let key = m.split('\'').nth(1).map(str::to_string);
        let d = d.kind(DiagKind::UnknownOption);
        return match key {
            Some(k) => d.key(k),
            None => d,
        };
    }
    if lower.contains("deprecated") {
        let key = m.split_whitespace().next().filter(|w| !w.contains(':')).map(str::to_string);
        let d = d.kind(DiagKind::Deprecated);
        return match key {
            Some(k) => d.key(k),
            None => d,
        };
    }
    if lower.contains("globbing error") || lower.contains("inaccessible") || (lower.contains("module") && lower.contains("not found")) || lower.contains("cannot open") || lower.contains("no such file") {
        return d.kind(DiagKind::MissingInclude);
    }
    if lower.contains("unexpected symbol") || lower.contains("expected near") || lower.contains("' expected") || lower.contains("unfinished") || lower.contains("unclosed") || lower.contains("malformed") || lower.contains("backslash") {
        return d.kind(DiagKind::SyntaxError);
    }
    if lower.contains("error setting") || lower.contains("invalid") || lower.contains("expected") || lower.contains("must be") || lower.contains("failed parsing") || lower.contains("failed to parse") || lower.contains("no such") || lower.contains("bad argument") || lower.contains("does not exist") {
        return d.kind(DiagKind::InvalidValue);
    }
    d
}
