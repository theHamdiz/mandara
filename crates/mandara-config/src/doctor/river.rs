//! River: no config checker exists (its config is an executable `init`
//! program), so the doctor checks what it can, never changing `init`:
//!
//! - the version (`river -version`, the "checker" run): River 0.4 moved
//!   window management into separate programs and dropped the
//!   `river-control` and `river-status` protocols Mandara drives River
//!   with; those need River's classic line (0.3, packaged as
//!   `river-classic` where both exist);
//! - that `init` is executable (River runs it as a program);
//! - that `init` starts Mandara (the one line Mandara asks for);
//! - the keys `init` maps that Mandara binds too (Mandara skips its own).
//!
//! Every finding is read-only: no comment / remove fixes, as Mandara
//! never edits `init`.

use std::ffi::OsString;
use std::path::Path;

use mandara_i18n::tf;

use super::{CompositorCheck, DiagKind, Diagnostic, Output, ParseContext, Severity};
use crate::compositor::{Target, river_binds, river_spawn_line, river_starts_mandara};

/// The first River version without `river-control` / `river-status`.
pub const RIVER_NON_CLASSIC: (u32, u32) = (0, 4);

pub struct River;

impl CompositorCheck for River {
    fn target(&self) -> Target {
        Target::River
    }

    fn program(&self) -> &'static str {
        "river"
    }

    fn args(&self, _config: &Path) -> Vec<OsString> {
        vec!["-version".into()]
    }

    fn version_flag(&self) -> &'static str {
        "-version"
    }

    fn parse(&self, out: &Output, ctx: &ParseContext) -> Vec<Diagnostic> {
        let mandara: Vec<String> = crate::ConfigStore::open_default().map(|s| s.get().compositor.keybinds.iter().map(|k| k.chord()).collect()).unwrap_or_default();
        let mandara_bin = crate::paths::self_exe().display().to_string();
        check(&out.text(), &ctx.root, &mandara, &mandara_bin)
    }

    fn rejected(&self, _out: &Output) -> bool {
        false
    }

    /// `riverctl map normal Super Q close` → `map`, the command a line runs.
    fn line_key(&self, line: &str) -> Option<String> {
        let words = crate::compositor::shell_words(line);
        let at = words.iter().position(|w| w == "riverctl" || w.ends_with("/riverctl"))?;
        words.get(at + 1).cloned()
    }
}

/// The checks, given `river -version`'s output, the `init` file, the
/// chords Mandara binds and the path that starts it.
pub(super) fn check(version_text: &str, init: &Path, mandara_chords: &[String], mandara_bin: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let read_only = |d: Diagnostic| Diagnostic { read_only: true, ..d };
    let file = init.display().to_string();
    if let Some((major, minor)) = crate::compositor::parse_version(version_text)
        && (major, minor) >= RIVER_NON_CLASSIC
    {
        let version = format!("{major}.{minor}");
        out.push(read_only(Diagnostic::new(Severity::Error, tf("River {version} runs window management in a separate program and no longer has the river-control and river-status protocols, so Mandara can't show or switch its tags, send its binds or follow its windows. Mandara works with River's classic line (0.3, packaged as river-classic).", &[("version", &version)])).kind(DiagKind::Other).key("river-control")));
    }
    let Ok(text) = std::fs::read_to_string(init) else { return out };
    {
        use std::os::unix::fs::PermissionsExt as _;
        let executable = std::fs::metadata(init).is_ok_and(|m| m.permissions().mode() & 0o111 != 0);
        if !executable {
            let mut d = Diagnostic::new(Severity::Error, tf("River runs {file} as a program, but it isn't executable, so River starts without it (chmod +x makes it run).", &[("file", &file)])).kind(DiagKind::Other);
            d.file = Some(init.to_path_buf());
            out.push(read_only(d));
        }
    }
    if !river_starts_mandara(&text) {
        let line = river_spawn_line(mandara_bin);
        let mut d = Diagnostic::new(Severity::Warning, tf("Your River init doesn't start Mandara. Mandara never edits it: add this line to it yourself: {line}", &[("line", &line)])).kind(DiagKind::Other);
        d.file = Some(init.to_path_buf());
        out.push(read_only(d));
    }
    for b in river_binds(&text) {
        if mandara_chords.contains(&b.chord) {
            let d = Diagnostic::new(Severity::Info, tf("Your init maps {chord}, which Mandara binds too: Mandara leaves yours and skips its own.", &[("chord", &b.chord)])).kind(DiagKind::BindConflict).key(b.chord.clone()).at(init, b.line + 1, None);
            out.push(read_only(d));
        }
    }
    out
}
